import fs from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import assert from 'node:assert/strict';
import {spawn,spawnSync} from 'node:child_process';
import {fileURLToPath} from 'node:url';
const {chromium,request}=await import(process.env.PLAYWRIGHT_MODULE||'playwright');
const root=fileURLToPath(new URL('../../',import.meta.url)),binary=process.env.AGENT_INSTANCE_BINARY||path.join(root,'target/debug/agent-instance');
const home=await fs.mkdtemp(path.join(os.tmpdir(),'development-ui-'));
let service,browser,context,api,page;const checks=[],errors=[];
function cli(...args){const r=spawnSync(binary,['--home',home,...args],{encoding:'utf8'});assert.equal(r.status,0,r.stderr);return JSON.parse(r.stdout);}
const pass=s=>{checks.push(s);console.log('PASS '+s);};
try{
 cli('init','--name','Development fixture','--bind','127.0.0.1');
 const doc=JSON.parse(await fs.readFile(home+'/config.json','utf8'));doc.config.setup_complete=true;
 await fs.mkdir(home+'/engine');await fs.copyFile(path.join(root,'tests/fixtures/native_account.py'),home+'/engine/agent-runtime');await fs.chmod(home+'/engine/agent-runtime',0o700);
 await fs.writeFile(home+'/engine/runtime.json',JSON.stringify({provider:{kind:'openai',model:'previous-model',endpoint:'https://api.openai.com/v1/responses',api_key_env:'OPENAI_API_KEY',auth:'auto',native:{}},mode:'ask',decisions:[]}));
 doc.config.backend={command:'${AGENT_HOME}/engine/agent-runtime',args:['--home','${AGENT_HOME}/engine','acp'],env:{},secret_env:{},working_directory:'${AGENT_HOME}/workspace',allowed_working_roots:['${AGENT_HOME}/workspace']};await fs.writeFile(home+'/config.json',JSON.stringify(doc),{mode:0o600});
 service=spawn(binary,['--home',home,'serve'],{stdio:'ignore'});let origin;
 for(let i=0;i<100;i++){try{origin=JSON.parse(await fs.readFile(home+'/endpoints.json','utf8')).base_url;break;}catch{}await new Promise(r=>setTimeout(r,100));}assert(origin);
 api=await request.newContext({ignoreHTTPSErrors:true,baseURL:origin,extraHTTPHeaders:{Authorization:'Bearer '+(await fs.readFile(home+'/secrets/control-token','utf8')).trim()}});
 async function call(method,p,data){const r=await api.fetch('/api/v1/'+p,{method,...(data===undefined?{}:{data})});assert(r.ok(),await r.text());return r.json();}

 let config=await call('GET','config');config.config.server.public_url=origin;await call('PUT','config',{expected_revision:config.revision,config:config.config});
 const seed=spawnSync('python3',['-c',"import sqlite3,hashlib,time,sys; db=sqlite3.connect(sys.argv[1]); db.execute('INSERT INTO browser_sessions VALUES(?,?,?,?)',(hashlib.sha256(b'native-oauth-fixture').hexdigest(),sys.argv[2],'fixture-csrf',int(time.time()*1000)+3600000)); db.commit()",home+'/state.sqlite3',doc.config.owner],{encoding:'utf8'});assert.equal(seed.status,0,seed.stderr);
 browser=await chromium.launch({headless:true,args:['--no-sandbox']});context=await browser.newContext({ignoreHTTPSErrors:true,viewport:{width:1400,height:1000}});
 await context.addCookies([{name:'ai_'+doc.config.instance_id.replaceAll('-',''),value:'native-oauth-fixture',url:origin,secure:true,httpOnly:true,sameSite:'Strict'}]);
 page=await context.newPage();page.on('pageerror',e=>errors.push(e.message));await page.goto(origin);await page.locator('[data-control="nav.connections"]').click();
 await page.locator('[data-action="development.open"]').waitFor();
 await page.locator('[data-action="development.open"]').click();await page.getByRole('heading',{name:'Connect GitHub',exact:true}).waitFor();
 await page.evaluate(()=>window.agentInstanceControl.perform('development.manual'));
 await page.locator('[data-control="development.credential"]').fill('private-browser-canary');
 let view=await page.evaluate(()=>window.agentInstanceControl.getState());assert(!JSON.stringify(view).includes('private-browser-canary'));
 assert.equal((await call('GET','development')).ready_to_develop,false);
 pass('Owner setup is discoverable and distinguishes configured tools from a verified build');
 await page.setViewportSize({width:390,height:844});assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));await page.setViewportSize({width:1400,height:1000});
 await page.evaluate(()=>window.agentInstanceControl.perform('modal.close'));
 let patched=false;for(let attempt=0;attempt<10;attempt++){
  const remote=await call('GET','views/'+view.id);remote.state.route='connections';remote.state.modal='development-github';remote.state.filters.development_manual=true;
  const r=await api.patch('/api/v1/views/'+view.id,{data:{expected_revision:remote.revision,state:remote.state}});if(r.status()===409)continue;assert(r.ok(),await r.text());patched=true;break;
 }assert(patched);await page.getByRole('heading',{name:'Connect GitHub',exact:true}).waitFor();
 assert.equal(await page.locator('[data-control="development.credential"]').inputValue(),'');
 await page.reload();await page.getByRole('heading',{name:'Connect GitHub',exact:true}).waitFor();
 assert.equal(await page.locator('[data-control="development.credential"]').inputValue(),'');
 pass('API puppeteering restores the view without recovering secret input; mobile layout fits');
 const status=cli('development','status');assert.equal(status.github.configured,false);assert.equal(status.environment.build_verified,false);
 await page.evaluate(()=>window.agentInstanceControl.perform('modal.close'));
 await page.evaluate(()=>window.agentInstanceControl.perform('development.check'));
 assert.deepEqual(errors,[]);pass('CLI, semantic actions, and real service status agree');
 console.log(JSON.stringify({passed:true,checks,external_requests:false}));
}finally{await context?.close();await browser?.close();await api?.dispose();if(service){service.kill();await new Promise(resolve=>{if(service.exitCode!==null)return resolve();const timer=setTimeout(()=>{service.kill('SIGKILL');resolve();},5000);service.once('exit',()=>{clearTimeout(timer);resolve();});});}await fs.rm(home,{recursive:true,force:true});}
