"""Constrain only the installation bridge; never change other Docker networks."""
import json, pathlib, subprocess, sys

def ip(*args, check=True):
    return subprocess.run(['/usr/sbin/iptables','-w','10',*args],check=check,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL).returncode

def main(path):
    config=json.loads(pathlib.Path(path).read_text())
    for chain in ['CONSTELLATION-OUT','CONSTELLATION-HOST']:
        ip('-N',chain,check=False)
        # Deny during refresh; existing traffic is not temporarily unrestricted.
        ip('-I',chain,'1','-j','DROP')
        # Remove the old rules beneath the temporary guard one by one.
        while ip('-D',chain,'2',check=False)==0:pass
        ip('-A',chain,'-m','conntrack','--ctstate','ESTABLISHED,RELATED','-j','ACCEPT')
        for agent in config['agents']:
            for address,port in agent.get('allow',[]):
                ip('-A',chain,'-s',agent['ip'],'-d',address,'-p','tcp','--dport',str(port),'-j','ACCEPT')
        if chain=='CONSTELLATION-OUT':
            for cidr in ['0.0.0.0/8','10.0.0.0/8','100.64.0.0/10','127.0.0.0/8','169.254.0.0/16','172.16.0.0/12','192.168.0.0/16','224.0.0.0/4','240.0.0.0/4']:
                ip('-A',chain,'-d',cidr,'-j','REJECT')
            ip('-A',chain,'-j','RETURN')
        else:ip('-A',chain,'-j','REJECT')
        ip('-D',chain,'1')
    for parent,target in [('DOCKER-USER','CONSTELLATION-OUT'),('INPUT','CONSTELLATION-HOST')]:
        args=['-s','172.30.240.0/24','-j',target]
        if ip('-C',parent,*args,check=False):ip('-I',parent,'1',*args)

if __name__=='__main__':main(sys.argv[1])
