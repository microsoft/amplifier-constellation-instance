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
 const serviceDefinition={id:'minecraft',label:'Minecraft',add:{revision:'template1',fields:[{name:'label',label:'Connection name',kind:'text',required:true,max_length:120},{name:'notify',label:'Notify me',kind:'boolean',required:true},{name:'server',label:'Server',kind:'choice',required:true,choices:[{value:'workshop',label:'Constellation Workshop'}]}]}};
 const connection={id:'world',service:'minecraft',label:'Home Server',revision:'r1',context_provider_ids:['context-home'],state:'connected',message:''};
 await call('POST','service-connections/report',{schema:1,services:[serviceDefinition],connections:[connection]});
 await page.locator('[data-control="nav.connections"]').click();
 await page.getByRole('heading',{name:'Saved connections'}).waitFor();
 await Promise.all([page.waitForResponse(r=>r.url().endsWith('/api/v1/service-connections') && r.request().method()==='POST'),page.getByRole('button',{name:'Disconnect',exact:true}).click()]);
 let state=await call('GET','service-connections');assert.equal(state.request.command.action,'disconnect');
 assert.equal(await page.getByRole('button',{name:'Connect',exact:true}).isEnabled(),false);
 pass('Disconnect is a durable request and does not pretend it completed');
 await call('POST','service-connections/report',{schema:1,services:[serviceDefinition],connections:[{...connection,state:'disconnected',request_id:state.request.command.request_id,settled:true}]});
 await page.evaluate(()=>window.agentInstanceControl.perform('refresh'));
 await Promise.all([page.waitForResponse(r=>r.url().endsWith('/api/v1/service-connections') && r.request().method()==='POST'),page.getByRole('button',{name:'Connect',exact:true}).click()]);
 state=await call('GET','service-connections');assert.equal(state.request.command.action,'connect');
 await call('POST','service-connections/report',{schema:1,services:[serviceDefinition],connections:[{...connection,request_id:state.request.command.request_id,settled:true}]});
 await page.evaluate(()=>window.agentInstanceControl.perform('refresh'));
 await page.getByRole('button',{name:'Add Minecraft connection'}).click();
 await page.locator('[data-control="service_add.label"]').fill('Workshop');
 await page.locator('[data-control="service_add.server"]').selectOption('workshop');
 await Promise.all([page.waitForResponse(r=>r.url().endsWith('/api/v1/service-connections') && r.request().method()==='POST'),page.getByRole('button',{name:'Save connection',exact:true}).click()]);
 state=await call('GET','service-connections');assert.equal(state.request.command.action,'add');assert.deepEqual(state.request.command.values,{label:'Workshop',notify:false,server:'workshop'});
 pass('Add renders owner-defined fields and submits only typed values');
 await call('POST','service-connections/report',{schema:1,services:[serviceDefinition],connections:[connection,{...connection,id:'workshop',label:'Workshop',context_provider_ids:['context-workshop'],state:'disconnected'}],add_result:{service_id:'minecraft',request_id:state.request.command.request_id,state:'saved',settled:true,connection_id:'workshop',message:'Saved'}});
 await page.evaluate(()=>window.agentInstanceControl.perform('refresh'));
 assert.equal(await page.getByRole('button',{name:'Connect',exact:true}).count(),2);
 assert.equal((await call('GET','service-connections')).request,null);
 await page.setViewportSize({width:390,height:844});assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));
 assert.deepEqual(errors,[]);pass('Two independent saved connections remain visible on mobile');
 console.log(JSON.stringify({passed:true,checks,external_requests:false}));
}finally{await context?.close();await browser?.close();await api?.dispose();if(service){service.kill();await new Promise(resolve=>{if(service.exitCode!==null)return resolve();const timer=setTimeout(()=>{service.kill('SIGKILL');resolve();},5000);service.once('exit',()=>{clearTimeout(timer);resolve();});});}await fs.rm(home,{recursive:true,force:true});}
