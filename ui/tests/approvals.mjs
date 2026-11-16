import fs from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import {spawn, spawnSync} from 'node:child_process';
import assert from 'node:assert/strict';
import {fileURLToPath} from 'node:url';
const {chromium, request}=await import(process.env.PLAYWRIGHT_MODULE||'playwright');
const project=fileURLToPath(new URL('../../',import.meta.url));
const binary=process.env.AGENT_INSTANCE_BINARY||path.join(project,'target/debug/agent-instance');
const home=await fs.mkdtemp(path.join(os.tmpdir(),'agent-instance-approval-ui-'));
const artifacts=process.env.UI_TEST_ARTIFACTS||await fs.mkdtemp(path.join(os.tmpdir(),'agent-instance-approval-results-'));
await fs.mkdir(artifacts,{recursive:true});
let service,browser,context,api,page,logs='';
const checks=[],errors=[],mutations=[];
const seed=code=>{
  const r=spawnSync('python3',['-c',code,home+'/state.sqlite3'],{encoding:'utf8'});
  assert.equal(r.status,0,r.stderr);
};
try {
  const init=spawnSync(binary,['--home',home,'init','--name','Approval fixture','--bind','127.0.0.1','--port','0'],{encoding:'utf8'});
  assert.equal(init.status,0,init.stderr);
  const doc=JSON.parse(await fs.readFile(home+'/config.json','utf8'));
  doc.config.setup_complete=true;
  doc.config.backend={command:'/usr/bin/cat',args:[],env:{},secret_env:{},working_directory:'${AGENT_HOME}/workspace',allowed_working_roots:['${AGENT_HOME}/workspace']};
  await fs.writeFile(home+'/config.json',JSON.stringify(doc),{mode:0o600});
  service=spawn(binary,['--home',home,'serve'],{stdio:['ignore','ignore','pipe']});
  service.stderr.on('data',b=>{logs=(logs+b).slice(-32768);});
  let origin;
  for(let i=0;i<100;i++){
    try{origin=JSON.parse(await fs.readFile(home+'/endpoints.json','utf8')).base_url;break;}catch{}
    assert.equal(service.exitCode,null,logs);
    await new Promise(r=>setTimeout(r,100));
  }
  assert(origin,logs);
  seed(`import sqlite3,hashlib,time,sys\ndb=sqlite3.connect(sys.argv[1]);db.execute('INSERT INTO browser_sessions(hash,owner,csrf,expires_at) VALUES(?,?,?,?)',(hashlib.sha256(b'approval-ui-fixture').hexdigest(),${JSON.stringify(doc.config.owner)},'approval-csrf',int(time.time()*1000)+3600000));db.commit()`);
  api=await request.newContext({ignoreHTTPSErrors:true,baseURL:origin,extraHTTPHeaders:{Authorization:'Bearer '+(await fs.readFile(home+'/secrets/control-token','utf8')).trim()}});
  const call=async(method,p,data)=>{const r=await api.fetch('/api/v1/'+p,{method,data});assert(r.ok(),await r.text());return r.json();};
  const get=()=>call('GET','approval-policy');
  const token=await call('POST','tokens',{name:'Workshop Slack',scopes:['acp'],expires_in_seconds:3600});
  const other=await call('POST','tokens',{name:'Another chat',scopes:['acp'],expires_in_seconds:3600});
  const configBefore=await call('GET','config');
  assert.equal((await get()).mode,'ask');
  assert.deepEqual((await get()).delegated_clients,[]);
  browser=await chromium.launch({headless:true,args:['--no-sandbox'],...(process.env.CHROMIUM_EXECUTABLE?{executablePath:process.env.CHROMIUM_EXECUTABLE}:{})});
  context=await browser.newContext({ignoreHTTPSErrors:true,viewport:{width:1400,height:1050}});
  await context.addCookies([{name:'ai_'+doc.config.instance_id.replaceAll('-',''),value:'approval-ui-fixture',url:origin,secure:true,httpOnly:true,sameSite:'Strict'}]);
  page=await context.newPage();
  page.on('pageerror',e=>errors.push(e.message));
  page.on('request',r=>{if(r.url().includes('/api/v1/approval-policy')&&r.method()!=='GET')mutations.push({method:r.method(),path:new URL(r.url()).pathname,body:r.postDataJSON()});});
  await page.goto(origin+'/#access');
  await page.getByRole('heading',{name:'Approvals',exact:true}).waitFor();
  await page.locator('[data-control="approvals.mode"]').waitFor();
  assert.equal(await page.locator('[data-control="approvals.mode"]').inputValue(),'ask');
  assert.equal(mutations.length,0);
  assert(await page.locator('[data-action="approvals.save"]').isDisabled());
  checks.push('Opening settings does not enable automatic approval or delegate a client');

  await page.locator('[data-control="approvals.mode"]').selectOption('automatic');
  await page.locator(`[data-approval-client="${token.id}"]`).check();
  assert.equal((await get()).mode,'ask');
  await page.locator('[data-action="approvals.save"]').click();
  await page.locator('#app').getByText('Approval settings saved for all chats with this agent.',{exact:true}).waitFor();
  assert.equal(mutations.length,1);
  assert.equal((await get()).mode,'automatic');
  assert.deepEqual((await get()).delegated_clients,[token.id]);
  assert.deepEqual(await call('GET','config'),configBefore);
  await page.reload();await page.locator('[data-control="approvals.mode"]').waitFor();
  assert.equal(await page.locator('[data-control="approvals.mode"]').inputValue(),'automatic');
  assert(await page.locator(`[data-approval-client="${token.id}"]`).isChecked());
  assert(!(await page.locator(`[data-approval-client="${other.id}"]`).isChecked()));
  checks.push('Explicit save persists across reload without changing engine settings or other clients');

  let releaseRead,enteredRead;
  const delayedRead=new Promise(r=>{releaseRead=r;}),readStarted=new Promise(r=>{enteredRead=r;});
  let holdRead=true;
  await page.route('**/api/v1/approval-policy',async route=>{
    if(route.request().method()==='GET'&&holdRead){holdRead=false;const response=await route.fetch();enteredRead();await delayedRead;await route.fulfill({response});}
    else await route.continue();
  });
  const refreshBeforeSave=page.evaluate(()=>window.agentInstanceControl.perform('refresh'));
  await readStarted;
  await page.locator('[data-control="approvals.mode"]').selectOption('ask');
  const beforeDouble=mutations.length;
  await page.evaluate(()=>Promise.all([window.agentInstanceControl.perform('approvals.save'),window.agentInstanceControl.perform('approvals.save')]));
  assert.equal(mutations.length,beforeDouble+1);
  assert.equal((await get()).mode,'ask');
  releaseRead();await refreshBeforeSave;
  assert.equal(await page.locator('[data-control="approvals.mode"]').inputValue(),'ask');
  assert(await page.locator('[data-action="approvals.save"]').isDisabled());
  await page.unroute('**/api/v1/approval-policy');
  await page.locator('[data-control="approvals.mode"]').selectOption('automatic');
  await page.evaluate(()=>window.agentInstanceControl.perform('approvals.save'));
  checks.push('An older read cannot undo a displayed save; duplicate save clicks produce only one mutation');

  await page.locator('[data-control="approvals.mode"]').selectOption('ask');
  let current=await get();
  await call('PUT','approval-policy',{expected_revision:current.revision,mode:'automatic',delegated_clients:[other.id]});
  const count=mutations.length;
  const conflict=await page.evaluate(async()=>{try{await window.agentInstanceControl.perform('approvals.save');return null;}catch(e){return e.status;}});
  assert.equal(conflict,409);
  assert.equal(mutations.length,count+1);
  assert(await page.locator('[data-action="approvals.save"]').isDisabled());
  assert.deepEqual((await get()).delegated_clients,[other.id]);
  await page.evaluate(()=>window.agentInstanceControl.perform('approvals.reload'));
  await page.locator(`[data-approval-client="${other.id}"]`).waitFor();
  assert.equal(await page.locator('[data-control="approvals.mode"]').inputValue(),'automatic');
  assert(await page.locator(`[data-approval-client="${other.id}"]`).isChecked());
  checks.push('Concurrent policy change returns one conflict and requires explicit review, without overwrite');

  await page.locator('[data-control="approvals.mode"]').selectOption('ask');
  await page.route('**/api/v1/approval-policy',async route=>{
    if(route.request().method()==='PUT'){await route.fetch();await route.abort('connectionreset');}
    else await route.continue();
  });
  const beforeLost=mutations.length;
  await page.evaluate(()=>window.agentInstanceControl.perform('approvals.save').catch(()=>{}));
  assert.equal((await get()).mode,'ask');
  assert.equal(mutations.length,beforeLost+1);
  await page.locator('#app').getByText('The save could not be confirmed. Reload approval settings before deciding what to do next.',{exact:true}).waitFor();
  assert(await page.locator('[data-action="approvals.save"]').isDisabled());
  await page.unroute('**/api/v1/approval-policy');
  await page.evaluate(()=>window.agentInstanceControl.perform('refresh'));
  assert(await page.locator('[data-action="approvals.save"]').isDisabled());
  await page.evaluate(()=>window.agentInstanceControl.perform('approvals.reload'));
  assert.equal(await page.locator('[data-control="approvals.mode"]').inputValue(),'ask');
  assert.equal(mutations.length,beforeLost+1);
  checks.push('A lost save response is not retried; reload reconciles the actual saved mode');

  // Seed only a display fixture in this disposable database. No engine approval
  // or effect is claimed by this browser test; ACP decision tests cover creation.
  seed(`import sqlite3,json,time,sys\ndb=sqlite3.connect(sys.argv[1]);p=json.loads(db.execute("SELECT value FROM records WHERE collection='approval_policy' AND id='policy'").fetchone()[0]);p['revision']+=1;p['rules']=[{'id':'fixture-rule','revision':p['revision'],'status':'active','backend':'not-the-current-engine','producer':'Fixture engine','key':'a'*64,'description':'Write <img src=x onerror=alert(1)> exactly','authority_lifetime':'request','creator':'token:${other.id}','created_at':int(time.time()*1000)}];db.execute("UPDATE records SET value=? WHERE collection='approval_policy' AND id='policy'",(json.dumps(p),));db.commit()`);
  await page.evaluate(()=>window.agentInstanceControl.perform('approvals.reload'));
  await page.locator('#app').getByText('Write <img src=x onerror=alert(1)> exactly',{exact:true}).waitFor();
  assert.equal(await page.locator('.approval-rules img').count(),0);
  await page.locator('#app').getByText(/Not currently usable · Fixture engine/).waitFor();
  await page.screenshot({path:path.join(artifacts,'approvals-desktop.png'),fullPage:true});
  await page.setViewportSize({width:390,height:844});
  assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));
  await page.screenshot({path:path.join(artifacts,'approvals-mobile.png'),fullPage:true});
  await page.locator('[data-action="approvals.revoke"]').click();
  await page.locator('#app').getByText('No remembered approvals',{exact:true}).waitFor();
  assert.equal((await get()).rules[0].status,'revoked');
  await page.reload();await page.locator('#app').getByText('No remembered approvals',{exact:true}).waitFor();
  checks.push('Remembered rules render as text, disclose unavailable bindings and can be durably revoked on mobile');

  await page.route('**/api/v1/approval-policy',route=>route.fulfill({status:503,contentType:'application/json',body:'{"error":"Approval store unavailable"}'}));
  await page.evaluate(()=>window.agentInstanceControl.perform('approvals.reload'));
  await page.locator('#app').getByText('Approval store unavailable',{exact:true}).waitFor();
  assert(await page.locator('[data-control="approvals.mode"]').isDisabled());
  assert(await page.locator('[data-action="approvals.save"]').isDisabled());
  await page.unroute('**/api/v1/approval-policy');
  assert.deepEqual(errors,[]);
  checks.push('Unavailable policy disables changes instead of presenting an editable assumed default');
  const report={passed:true,checks,scope:'Actual bundled UI and Rust policy API on disposable loopback installation; synthetic owner login and one display rule; no model or live connector'};
  await fs.writeFile(path.join(artifacts,'result.json'),JSON.stringify(report,null,2)+'\n');console.log(JSON.stringify(report));
} catch(e) {
  if(page){await page.screenshot({path:path.join(artifacts,'failure.png'),fullPage:true});console.error(await page.locator('[role="alert"]').allTextContents());}
  throw e;
} finally {
  await context?.close();await browser?.close();await api?.dispose();
  if(service&&service.exitCode===null){service.kill('SIGTERM');await new Promise((resolve,reject)=>{const timer=setTimeout(()=>{service.kill('SIGKILL');},10000);service.once('exit',()=>{clearTimeout(timer);resolve();});service.once('error',reject);});}
  await fs.rm(home,{recursive:true,force:true});
}
