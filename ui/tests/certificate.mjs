// Real browser trust, using a temporary HOME/NSS store; never changes the user's certificates.
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import assert from 'node:assert/strict';
import {spawn,spawnSync} from 'node:child_process';
import {fileURLToPath} from 'node:url';
const {chromium}=await import(process.env.PLAYWRIGHT_MODULE || 'playwright');
const repo=fileURLToPath(new URL('../../',import.meta.url));
const binary=process.env.AGENT_INSTANCE_BINARY || path.join(repo,'target/debug/agent-instance');
const scratch=await fs.mkdtemp(path.join(os.tmpdir(),'agent-cert-browser-'));
const home=path.join(scratch,'instance'),browserHome=path.join(scratch,'browser-home');
const artifacts=process.env.UI_TEST_ARTIFACTS || await fs.mkdtemp(path.join(os.tmpdir(),'agent-cert-results-'));
await fs.mkdir(artifacts,{recursive:true});await fs.mkdir(browserHome,{recursive:true});
const run=(cmd,args)=>{const result=spawnSync(cmd,args,{encoding:'utf8'});assert.equal(result.status,0,result.stderr);return result.stdout;};
run(binary,['init','--home',home,'--name','Certificate browser fixture','--bind','127.0.0.1']);
const service=spawn(binary,['serve','--home',home],{stdio:['ignore','ignore','pipe']});let log='';service.stderr.on('data',b=>log+=b.toString());
let browser;
const launch=()=>chromium.launch({headless:true,executablePath:process.env.CHROMIUM_EXECUTABLE,env:{...process.env,HOME:browserHome,XDG_CONFIG_HOME:path.join(browserHome,'.config'),XDG_DATA_HOME:path.join(browserHome,'.local/share')},args:['--no-sandbox']});
const checks=[];
try {
 let endpoints;
 for(let i=0;i<100;i++){try{endpoints=JSON.parse(await fs.readFile(path.join(home,'endpoints.json'),'utf8'));break;}catch{await new Promise(r=>setTimeout(r,100));}}
 assert(endpoints,log);assert.match(endpoints.welcome_url,/^http:/);
 for(const resource of ['/api/v1/status','/api/v1/auth/login','/tls/key.pem','/tls/root-ca.key']){const r=await fetch(endpoints.welcome_url+resource);assert.equal(r.status,404);}
 checks.push('HTTP welcome exposes no management, login or private keys');
 browser=await launch();let context=await browser.newContext({ignoreHTTPSErrors:false,acceptDownloads:true,viewport:{width:1050,height:1000}});let page=await context.newPage();
 await assert.rejects(()=>page.goto(endpoints.base_url),/ERR_CERT_AUTHORITY_INVALID/);
 checks.push('Fresh browser rejects the untrusted TLS chain');
 await page.close();page=await context.newPage();
 await page.goto(endpoints.welcome_url);await page.locator('#ready').waitFor({state:'visible'});
 await page.waitForFunction(()=>document.querySelector('#check-result').textContent.startsWith('We couldn’t confirm'));
 assert.equal(await page.locator('input[type=password]').count(),0);
 const downloadWait=page.waitForEvent('download');await page.locator('#download').click();const download=await downloadWait;
 const downloaded=path.join(scratch,download.suggestedFilename());await download.saveAs(downloaded);
 assert.deepEqual(await fs.readFile(downloaded),await fs.readFile(path.join(home,'tls/root-ca.crt')));
 for(const platform of ['windows','mac','ubuntu']){await page.evaluate(p=>window.agentInstanceWelcome.selectPlatform(p),platform);assert(await page.locator('#'+platform).isVisible());}
 await page.evaluate(()=>window.agentInstanceWelcome.selectPlatform('mac'));
 await page.screenshot({path:path.join(artifacts,'certificate-guide.png'),fullPage:true});
 checks.push('Download matches public CA and all three OS guides are accessible and controllable');
 await browser.close();browser=null;
 // Populate both layouts supported across Chromium versions, within this disposable HOME only.
 for(const db of ['.pki/nssdb','.local/share/pki/nssdb']){
   const directory=path.join(browserHome,db);await fs.mkdir(directory,{recursive:true});
   if(!await fs.stat(path.join(directory,'cert9.db')).catch(()=>null))run('certutil',['-N','--empty-password','-d','sql:'+directory]);
   run('certutil',['-A','-d','sql:'+directory,'-t','C,,','-n','agent-instance-test','-i',downloaded]);
 }
 browser=await launch();context=await browser.newContext({ignoreHTTPSErrors:false});page=await context.newPage();
 const secure=await page.goto(endpoints.base_url);assert.equal(secure.status(),200);assert(await page.locator('input[type=password]').isVisible());
 checks.push('After importing the downloaded CA, real Chromium opens HTTPS without an exception');
 await page.goto(endpoints.welcome_url);await page.waitForFunction(()=>document.querySelector('#check-result').textContent.startsWith('This browser can open'));
 checks.push('Public HTTPS probe succeeds from HTTP after trust installation');
 await page.locator('#open').click();await page.waitForURL(endpoints.base_url+'/');
 checks.push('Welcome continues to the secure sign-in page');
 const report={checks,certificate_store:'isolated temporary NSS databases only',browser:'Chromium; Windows/macOS steps verified against vendor documentation, not executed on those operating systems'};
 await fs.writeFile(path.join(artifacts,'result.json'),JSON.stringify(report,null,2));console.log(JSON.stringify({...report,artifacts}));
} finally {
 if(browser)await browser.close();service.kill('SIGTERM');await new Promise(r=>service.once('exit',r));await fs.rm(scratch,{recursive:true,force:true});
}
