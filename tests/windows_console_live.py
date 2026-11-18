"""Actual Windows console/stream witnesses; no accounts, services or model calls.

The positive baseline is a console-subsystem Python child created by a detached
parent without CREATE_NO_WINDOW. It establishes allocation behavior, not a prior
observed consumer flash. Test-owned consoles are hidden using STARTUPINFO.
The product binary must be GUI-subsystem; interactive output joins an existing
parent console, whereas captured and file-redirected output is preserved.
"""
import argparse
import ctypes
from ctypes import wintypes
import hashlib
import json
import os
from pathlib import Path
import struct
import subprocess
import sys
import tempfile


def save(path,value):
    path.write_text(json.dumps(value,indent=2)+'\n',encoding='utf-8')

def windows():
    kernel=ctypes.WinDLL('kernel32',use_last_error=True)
    kernel.GetConsoleWindow.restype=wintypes.HWND
    kernel.GetStdHandle.argtypes=[wintypes.DWORD];kernel.GetStdHandle.restype=wintypes.HANDLE
    user=ctypes.WinDLL('user32',use_last_error=True)
    user.IsWindowVisible.argtypes=[wintypes.HWND];user.IsWindowVisible.restype=wintypes.BOOL
    return kernel,user

def observed():
    kernel,user=windows();window=kernel.GetConsoleWindow()
    return {'pid':os.getpid(),'console_window':int(window or 0),'console_visible':bool(window and user.IsWindowVisible(window))}

def hidden_startup():
    startup=subprocess.STARTUPINFO();startup.dwFlags|=subprocess.STARTF_USESHOWWINDOW;startup.wShowWindow=0
    return startup

def probe():
    value=observed();value['input']=sys.stdin.read();sys.stdout.write(json.dumps(value)+'\n');sys.stdout.flush();sys.stderr.write('fixture-stderr-preserved\n')

def detached(binary,report):
    assert observed()['console_window']==0
    rows=[]
    for flags,label,expected in [(0,'unprotected detached-parent baseline',True),(subprocess.CREATE_NO_WINDOW,'windowless captured helper',False)]:
        result=subprocess.run([sys.executable,'-X','utf8',__file__,'--role','probe'],input='fixture café 日本\n',capture_output=True,text=True,encoding='utf-8',timeout=15,creationflags=flags,startupinfo=hidden_startup())
        assert result.returncode==0,result.stderr
        value=json.loads(result.stdout);assert bool(value['console_window']) is expected,value
        assert value['input']=='fixture café 日本\n';assert result.stderr=='fixture-stderr-preserved\n';value['case']=label;rows.append(value)
    version=subprocess.run([str(binary),'--version'],capture_output=True,text=True,encoding='utf-8',timeout=15)
    assert version.returncode==0,version.stderr;assert version.stdout.startswith('agent-instance ')
    refused=subprocess.run([str(binary),'--unknown-console-fixture-option'],capture_output=True,text=True,encoding='utf-8',timeout=15)
    assert refused.returncode!=0;assert 'error' in refused.stderr.lower();assert refused.stdout==''
    with tempfile.TemporaryDirectory(prefix='instance-console-redirect-') as tmp:
        out=Path(tmp)/'stdout';err=Path(tmp)/'stderr'
        with out.open('wb') as stdout,err.open('wb') as stderr:
            result=subprocess.run([str(binary),'--version'],stdin=subprocess.DEVNULL,stdout=stdout,stderr=stderr,timeout=15)
        assert result.returncode==0;assert out.read_text().startswith('agent-instance ');assert err.read_bytes()==b''
    save(report,{'passed':True,'observations':rows,'captured_version':version.stdout.strip(),'captured_error_exit':refused.returncode,'file_redirects_preserved':True})

class COORD(ctypes.Structure):
    _fields_=[('X',ctypes.c_short),('Y',ctypes.c_short)]

def interactive(binary,report):
    kernel,_=windows();before=observed();assert before['console_window']!=0;assert not before['console_visible']
    # No pipe or file overrides: exercise GUI startup with inherited existing
    # console handles, which an all-handles-absent-only guard would miss.
    result=subprocess.run([str(binary),'--version'],timeout=15)
    assert result.returncode==0
    output=kernel.GetStdHandle(wintypes.DWORD(-11 & 0xffffffff));text=ctypes.create_unicode_buffer(8192);read=wintypes.DWORD()
    kernel.ReadConsoleOutputCharacterW.argtypes=[wintypes.HANDLE,wintypes.LPWSTR,wintypes.DWORD,COORD,ctypes.POINTER(wintypes.DWORD)]
    kernel.ReadConsoleOutputCharacterW.restype=wintypes.BOOL
    assert kernel.ReadConsoleOutputCharacterW(output,text,8192,COORD(0,0),ctypes.byref(read)),ctypes.get_last_error()
    assert 'agent-instance ' in text[:read.value],repr(text[:read.value])
    after=observed();assert after['console_window']==before['console_window']
    save(report,{'passed':True,'parent_before':before,'parent_after':after,'existing_console_output_observed':True})

def pe_subsystem(binary):
    with binary.open('rb') as source:
        assert source.read(2)==b'MZ';source.seek(0x3c);offset=struct.unpack('<I',source.read(4))[0];source.seek(offset)
        assert source.read(4)==b'PE\0\0';source.seek(offset+24+68);return struct.unpack('<H',source.read(2))[0]

def main():
    parser=argparse.ArgumentParser();parser.add_argument('--binary',type=Path);parser.add_argument('--output',type=Path);parser.add_argument('--role',choices=['probe','detached','interactive']);parser.add_argument('--report',type=Path);args=parser.parse_args()
    if os.name!='nt':raise SystemExit('This fixture must actually execute on Windows')
    if args.role=='probe':probe();return
    if args.role=='detached':detached(args.binary,args.report);return
    if args.role=='interactive':
        try:interactive(args.binary,args.report)
        except BaseException as error:
            save(args.report,{'passed':False,'error':repr(error)});raise
        return
    binary=args.binary.resolve();assert pe_subsystem(binary)==2,'Native desktop/service entry must be GUI-subsystem'
    with tempfile.TemporaryDirectory(prefix='instance-console-witness-') as tmp:
        root=Path(tmp);results={}
        for role,flags in [('detached',subprocess.CREATE_NO_WINDOW),('interactive',subprocess.CREATE_NEW_CONSOLE)]:
            report=root/(role+'.json')
            # The interactive fixture's own console is hidden; product code
            # never calls AllocConsole and the fixture creates no OS tasks.
            capture={'capture_output':True,'text':True} if role=='detached' else {}
            child=subprocess.run([sys.executable,'-X','utf8',__file__,'--role',role,'--binary',str(binary),'--report',str(report)],timeout=60,creationflags=flags,startupinfo=hidden_startup(),**capture)
            assert child.returncode==0,(role,child.stdout,child.stderr,report.read_text() if report.exists() else 'no report')
            results[role]=json.loads(report.read_text(encoding='utf-8'));assert results[role]['passed']
        result={'passed':True,'binary_sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),'pe_subsystem':2,'results':results,'scope':'Actual Windows captured helper/CLI and existing hidden-console output. OS service startup and ACP lifetimes use their separate existing native fixtures.'}
        args.output.parent.mkdir(parents=True,exist_ok=True);save(args.output,result);print(json.dumps(result))
if __name__=='__main__':main()
