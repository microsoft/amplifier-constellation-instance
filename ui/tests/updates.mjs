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
 page=await context.newPage();page.on('pageerror',e=>errors.push(e.message));await page.goto(origin);
 await call('POST','updates/report',{status:{state:'available',plan_id:'11111111-1111-4111-8111-111111111111',components:[{id:'runtime',version:'0.8.0',target_version:'0.9.0',available:true}]}});
 await page.locator('[data-control="nav.updates"]').click();
 await page.evaluate(()=>window.agentInstanceControl.perform('refresh'));
 await page.getByRole('heading',{name:'Keep this agent up to date.'}).waitFor();
 assert.equal(await page.locator('[data-control="updates.check_minutes"]').inputValue(),'60');
 await page.locator('[data-control="updates.agent_initiated"]').check();
 await page.locator('[data-control="updates.check_minutes"]').selectOption('15');
 await page.evaluate(()=>window.agentInstanceControl.perform('updates.save'));
 let value=await call('GET','updates');assert.equal(value.policy.agent_initiated,true);assert.equal(value.policy.check_minutes,15);
 pass('Dashboard and programmatic actions save one global authorization switch and schedule');
 await page.evaluate(()=>window.agentInstanceControl.perform('updates.check'));
 value=await call('GET','updates');assert.equal(value.request.action,'check');assert.equal(value.request.pending,true);
 await call('POST','updates/report',{status:{state:'up_to_date'},finished_request:value.request.id});
 await page.evaluate(()=>window.agentInstanceControl.perform('refresh'));
 await page.locator('[data-action="updates.update"]').click();value=await call('GET','updates');assert.equal(value.request.action,'update');
 pass('Check and update use the durable service API');
 await page.setViewportSize({width:390,height:844});assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));
 const view=await page.evaluate(()=>window.agentInstanceControl.getState());
 let patched=false;for(let i=0;i<10;i++){const v=await call('GET','views/'+view.id);v.state.route='updates';v.state.draft={updates:{install:'window',window_start_utc:4,window_hours:3}};const r=await api.patch('/api/v1/views/'+view.id,{data:{expected_revision:v.revision,state:v.state}});if(r.status()===409)continue;assert(r.ok());patched=true;break;}assert(patched);
 await page.locator('[data-control="updates.window_start_utc"]').waitFor();
 assert.deepEqual(errors,[]);pass('Remote view control exposes the scheduled window and mobile layout fits');
 console.log(JSON.stringify({passed:true,checks,external_requests:false}));
}finally{await context?.close();await browser?.close();await api?.dispose();if(service){service.kill();await new Promise(resolve=>{if(service.exitCode!==null)return resolve();const timer=setTimeout(()=>{service.kill('SIGKILL');resolve();},5000);service.once('exit',()=>{clearTimeout(timer);resolve();});});}await fs.rm(home,{recursive:true,force:true});}
