import http from 'node:http';
import fs from 'node:fs/promises';
import assert from 'node:assert/strict';
import os from 'node:os';
import path from 'node:path';
import {createHash} from 'node:crypto';
import { fileURLToPath } from 'node:url';
const { chromium } = await import(process.env.PLAYWRIGHT_MODULE || 'playwright');
const artifacts = process.env.UI_TEST_ARTIFACTS || await fs.mkdtemp(path.join(os.tmpdir(), 'agent-instance-ui-fixture-'));
await fs.mkdir(artifacts, { recursive: true });
const dir=fileURLToPath(new URL('../', import.meta.url));
const prefix='/embedded/alpha/';
const viewIntentOnly=process.env.UI_TEST_VIEW_INTENT_ONLY==='1';
const requests=[], streams=new Set(), events=[], missingRoutes=[];
const config={schema_version:1,instance_id:'fixture-instance',name:'New agent',purpose:'',instructions:'',attention:null,owner:'fixture-owner',setup_complete:false,enabled:false,server:{bind:'0.0.0.0',port:41411,welcome_port:41412,public_url:null,pam_service:'agent-instance',embed_origins:[],certificate_file:null,private_key_file:null},backend:{command:'/agent/runtime/bin/codex-acp',args:[],env:{},secret_env:{},working_directory:'${AGENT_HOME}/workspace',allowed_working_roots:['${AGENT_HOME}/workspace']},limits:{max_connections:8,max_frame_bytes:4194304,backend_start_timeout_seconds:30,idle_timeout_seconds:1800,event_retention:10000}};
let revision=1, loggedIn=false, view=null, profiles={}, secrets={}, tokens=[], sequence=0, accountReady=false, accountState='idle', runtimeInstalled=true;
let profilePackage={revision:0,active:null,previous:[]};
let delayedProfileApply=null,delayedProfileWrite=null;
let beforeViewWrite=null;
if(viewIntentOnly){config.setup_complete=true;config.enabled=true;loggedIn=true;accountReady=true;}
const profileSnapshots=new Map();
const digest=text=>createHash('sha256').update(text).digest('hex');
function saveProfileVersion(source=null,kind='manual'){
 const files={'communication.md':profiles.communication||'','working-style.md':profiles['working-style']||''};
 if(profilePackage.active)profilePackage.previous.unshift(profilePackage.active);
 const active={id:`snapshot-${profilePackage.revision+1}`,source,kind,created_at:Date.now(),files:Object.fromEntries(Object.entries(files).map(([name,text])=>[name,digest(text)]))};
 profileSnapshots.set(active.id,{...files});profilePackage={revision:profilePackage.revision+1,active,previous:profilePackage.previous};
}
function fixturePackage(revision,communication,working){
 const files={'communication.md':communication,'working-style.md':working,'claims.json':'{"evidence":"private-import-evidence-marker"}'};
 return {schema:'twin-profile/1',manifest:{schema:'twin-profile/1',instruction_format:2,subject_key:'fixture-persona',revision,required_context:['communication.md','working-style.md'],files:Object.fromEntries(Object.entries(files).map(([name,text])=>[name,digest(text)])),raw_evidence_included:false,identity_included:false},files};
}
function emit(kind,data,projection={}){const e={seq:++sequence,at:Date.now(),kind,data,...projection};events.push(e);for(const res of streams)res.write(`data: ${JSON.stringify(e)}\n\n`);}
emit('acp.prompt.started',{session_id:'session-one'},{summary:'Started working on a request',category:'work',severity:'info'});
emit('acp.prompt.finished',{session_id:'session-one',duration_ms:2100},{summary:'Completed a request',category:'outcomes',severity:'info'});
emit('acp.prompt.failed',{session_id:'session-one'},{summary:'A request needs attention',category:'problems',severity:'error'});
for(let n=0;n<1200;n++)emit('view.ack',{id:'older-view',revision:n},{summary:'Display update acknowledged',category:'diagnostics',severity:'info'});
function setPath(o,path,value){const keys=path.split('.');for(const k of keys.slice(0,-1))o=o[k]??={};o[keys.at(-1)]=value;}
function apply(c){const s=view.state;switch(c.type){case'navigate':s.route=c.route;if(c.step!==undefined)s.step=c.step;break;case'set_draft':setPath(s.draft,c.path,c.value);break;case'focus':s.focus=c.target;break;case'scroll':s.scroll={x:c.x,y:c.y};break;case'select_session':s.selected_session=c.session_id;break;case'set_filter':s.filters[c.key]=c.value;break;case'open_modal':s.modal=c.modal;break;case'close_modal':s.modal=null;break;default:throw Error('bad command '+c.type);}view.revision++;emit('view.changed',{id:view.id,revision:view.revision});}
const server=http.createServer(async(req,res)=>{
 const url=new URL(req.url,'http://localhost'),path=url.pathname;let body='';for await(const c of req)body+=c;const data=body?JSON.parse(body):null;
 requests.push({path,query:url.search,method:req.method,body:data,csrf:req.headers['x-csrf-token']});
 const send=(v,status=200)=>{res.writeHead(status,{'Content-Type':'application/json'});res.end(JSON.stringify(v));};
 if(!path.startsWith(prefix))return send({error:'Wrong prefix'},404);
 let endpoint=path.slice(prefix.length);
 if(!endpoint.startsWith('api/v1/')){const file=endpoint||'index.html';try{const text=await fs.readFile(file==='app.js'&&process.env.UI_TEST_APP_JS?process.env.UI_TEST_APP_JS:`${dir}/${file}`);res.writeHead(200,{'Content-Type':file.endsWith('.js')?'text/javascript':file.endsWith('.css')?'text/css':'text/html','Content-Security-Policy':"default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' data:"});return res.end(text);}catch{return send({error:'missing'},404);}}
 endpoint=endpoint.slice(7);
 if(endpoint==='auth/login'){loggedIn=true;return send({owner:'fixture-owner',csrf:'fixture-csrf'});}
 if(endpoint==='auth/session')return send(loggedIn?{owner:'fixture-owner',csrf:'fixture-csrf'}:{error:'Sign in'},loggedIn?200:401);
 if(!loggedIn)return send({error:'Unauthorized'},401);
 if(req.method!=='GET'&&req.headers['x-csrf-token']!=='fixture-csrf')return send({error:'Missing CSRF'},403);
 if(endpoint==='auth/logout'){loggedIn=false;return send({ok:true});}
 if(endpoint==='config'){if(req.method==='PUT'){if(data.expected_revision!==revision)return send({error:'Config conflict'},409);Object.assign(config,data.config);revision++;}return send({revision,config});}
 if(endpoint==='updates')return send({policy:{enabled:false},components:[],history:[]});
 if(endpoint==='service-connections')return send({schema:1,services:[],connections:[],observed_at:0,helper_connected:false,request:null,last_operation:null});
 if(endpoint==='development')return send({config_revision:revision,github:{configured:false,verification:null,local_clients_enabled:false,credential_boundary:'GitHub access follows the connected credential. Tool selection does not restrict repositories.'},environment:{kind:'external_acp_backend',build_verified:false,message:'Execution is provided by the connected agent. A project build has not been verified.'},ready_to_develop:false});
 if(endpoint==='schema')return send({type:'object'});
 if(endpoint==='identity')return send({avatar_revision:0,avatar:{kind:'default'}});
 if(endpoint==='identity/avatar'){res.writeHead(200,{'Content-Type':'image/png'});return res.end(Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a8ZUAAAAASUVORK5CYII=','base64'));}
 if(endpoint==='backend')return send({kind:config.backend?'codex':'none',configured:!!config.backend,installed:runtimeInstalled,name:'Codex',version:'fixture',auth_ready:accountReady,auth_validated:false,status:!runtimeInstalled?'missing_runtime':accountReady?'ready':'needs_account',account_source:accountReady?'instance_sign_in':'none',problem:null,help:null,actions:!runtimeInstalled?['install']:accountReady?[]:['connect_account']});
 if(endpoint==='backend/install'){runtimeInstalled=true;config.backend={command:'/agent/runtime/bin/codex-acp',args:[],env:{},secret_env:{},working_directory:'${AGENT_HOME}/workspace',allowed_working_roots:['${AGENT_HOME}/workspace']};revision++;return send({installed:true});}
 if(endpoint==='backend/account/login'){
  if(req.method==='POST'){accountState='pending';return send({state:'pending',verification_url:'https://auth.openai.com/codex/device',user_code:'FIXTURE-CODE',login_id:'fixture-login'});}
  if(req.method==='DELETE'){accountState='cancelled';return send({state:'cancelled'});}
  if(accountState==='pending'){accountReady=true;accountState='connected';}
  return send({state:accountState});
 }
 if(endpoint==='activity'){
  const diagnostics=url.searchParams.get('diagnostics')==='true',category=url.searchParams.get('category')||'all',search=(url.searchParams.get('search')||'').toLowerCase(),before=Number(url.searchParams.get('before')||Infinity),limit=Number(url.searchParams.get('limit')||100);
  const matching=events.map(e=>({...e,category:e.category||'diagnostics',summary:e.summary||'Display state updated',severity:e.severity||'info'})).filter(e=>e.seq<before&&(diagnostics||e.category!=='diagnostics')&&(category==='all'||e.category===category)&&(!search||JSON.stringify(e).toLowerCase().includes(search))).reverse();
  const items=matching.slice(0,limit);return send({items,has_more:matching.length>limit,next_before:matching.length>limit?items.at(-1).seq:null});
 }

 if(endpoint==='profile-use'){const configured=Object.values(profiles).some(text=>text.trim());return send({status:configured?'unknown':'not_configured',installed:{configured,package:{id:profilePackage.active?.id||null}},relevant_turn:null,behavior_quality:'not_evaluated'});}
 if(endpoint==='status')return send({instance_id:config.instance_id,name:config.name,enabled:config.enabled,setup_complete:config.setup_complete,version:'fixture',authentication:process.env.UI_TEST_AUTH_METHOD||'pam',uptime_ms:61000,config_revision:revision,addresses:{urls:['https://agent-host:41411/'],acp_attach:'agent-instance acp --home /agent',certificate_sha256:'fixture-fingerprint',welcome_url:'http://localhost:41412',welcome_urls:['http://localhost:41412','http://127.0.0.1:41412'],effective_server:config.server}});
 if(endpoint==='events/stream'){res.writeHead(200,{'Content-Type':'text/event-stream','Cache-Control':'no-cache','Connection':'keep-alive'});streams.add(res);res.write(':connected\n\n');req.on('close',()=>streams.delete(res));return;}
 if(endpoint==='events')return send({events:events.filter(e=>e.seq>Number(new URL(req.url,'http://localhost').searchParams.get('after')||0))});
 if(endpoint==='context/providers')return send({providers:[{id:'slack:fixture',label:'Workshop Slack',configured:true},{id:'web:fixture',label:'Conversation web app',configured:false}]});
 if(endpoint==='connections')return send([{id:'connection-one',client_name:'Example ACP client',transport:'stdio',state:'connected'}]);
 if(endpoint==='sessions')return send([{id:'session-one',title:'A useful task',last_connection_id:'connection-one',cwd:'/agent/work',status:'idle'}]);
 if(endpoint==='sessions/session-one')return send({id:'session-one',cwd:'/agent/work',state:'idle',model:'example'});
 if(endpoint.startsWith('profiles/')){const name=endpoint.slice(9);if(req.method==='PUT'){if(delayedProfileWrite){const pending=delayedProfileWrite;delayedProfileWrite=null;pending.entered();await pending.resume;}profiles[name]=data.text;if(name!=='identity')saveProfileVersion();}return send({text:profiles[name]||''});}
 if(endpoint==='profile-package'||endpoint==='profile-package/restore'){
  if(req.method==='POST'){
   if(delayedProfileApply){const pending=delayedProfileApply;delayedProfileApply=null;pending.entered();await pending.resume;}
   if(data.expected_revision!==profilePackage.revision)return send({error:'Profile version conflict'},409);
   if(endpoint.endsWith('/restore')){
    const files=profileSnapshots.get(data.id);if(!files)return send({error:'Unknown profile version'},404);
    const prior=profilePackage.previous.find(version=>version.id===data.id);
    profiles.communication=files['communication.md'];profiles['working-style']=files['working-style.md'];saveProfileVersion(prior?.source,prior?.kind);
   }else{
    profiles.communication=data.package.files['communication.md'];profiles['working-style']=data.package.files['working-style.md'];
    saveProfileVersion({schema:data.package.schema,subject_key:data.package.manifest.subject_key,revision:data.package.manifest.revision},'import');
   }
  }
  return send(profilePackage);
 }
 if(endpoint==='secrets')return send(Object.keys(secrets));
 if(endpoint.startsWith('secrets/')){const name=endpoint.slice(8);if(req.method==='DELETE')delete secrets[name];else secrets[name]=data.value;return send({ok:true});}
 if(endpoint==='tokens'){if(req.method==='POST'){tokens.push({id:'token-one',name:data.name,scopes:data.scopes,expires_at:Date.now()+3600000});return send({id:'token-one',token:'fixture-one-time-token'});}return send(tokens);}
 if(endpoint.startsWith('tokens/')){tokens=[];return send({ok:true});}
 if(endpoint==='lifecycle'){config.enabled=data.action==='start';revision++;return send({enabled:config.enabled});}
 if(endpoint==='views'&&req.method==='POST'){view={id:'view-one',revision:1,state:{route:config.setup_complete?'dashboard':'setup',step:0,selected_session:null,filters:{},draft:{},focus:null,scroll:{x:0,y:0},modal:null}};return send(view);}
 if(beforeViewWrite && (endpoint==='views/view-one'&&req.method==='PATCH'||endpoint==='views/view-one/commands')){const hook=beforeViewWrite;if(!hook.matches||hook.matches(data,endpoint)){beforeViewWrite=null;await hook(data,endpoint);}}
 if(endpoint==='views/view-one'){if(req.method==='PATCH'){if(data.expected_revision!==view.revision)return send({error:'View conflict'},409);view.state=data.state;view.revision++;emit('view.changed',{id:view.id,revision:view.revision});}return send(view);}
 if(endpoint==='views/view-one/commands'){if(data.expected_revision!==view.revision)return send({error:'View conflict'},409);apply(data.command);return send(view);}
 if(endpoint==='views/view-one/ack'){view.ack=data;return send(view);}
 missingRoutes.push(endpoint);return send({error:`No fixture ${endpoint}`},404);
});
await new Promise(r=>server.listen(0,'127.0.0.1',r));
const origin=`http://127.0.0.1:${server.address().port}`;
const browser=await chromium.launch({headless:true,...(process.env.CHROMIUM_EXECUTABLE ? {executablePath:process.env.CHROMIUM_EXECUTABLE} : {}),args:['--no-sandbox']});
const page=await browser.newPage({viewport:{width:1440,height:1050}});const errors=[];page.on('pageerror',e=>errors.push(e.message));page.on('console',m=>{if(m.type()==='error'&&!m.text().includes('401')&&!m.text().includes('409'))errors.push(m.text());});
try{
 if(viewIntentOnly){await page.goto(origin+prefix);await page.locator('[data-control="nav.configuration"]').waitFor();for(let n=0;!streams.size&&n<100;n++)await page.waitForTimeout(20);assert(streams.size,'Fixture event stream connected');}
 else {
 await page.goto(origin+prefix);assert.equal(await page.locator('[data-control="certificate.help"]').getAttribute('href'),'welcome');await page.locator('input[name=username]').fill('fixture-owner');await page.locator('input[name=password]').fill('fixture-login-password');await page.locator('[data-action="auth.login"]').click();
 assert.equal(await page.locator('[data-draft="config.backend.command"]').count(),0);
 await page.locator('[data-draft="config.name"]').fill('Atlas');await page.locator('[data-draft="config.purpose"]').fill('Explore useful ideas with a small team.');await page.locator('[data-action="setup.next"]').click();
 await page.locator('[data-action="backend.account.connect"]').waitFor();assert.equal(await page.locator('.wizard-steps .step-button').count(),2);assert(await page.locator('[data-action="setup.finish"]').isDisabled());assert.equal(await page.locator('[data-draft="config.backend.command"]').count(),0);
 // An older browser's four-step setup position migrates to Ready on reload.
 apply({type:'set_draft',path:'config.backend.command',value:''});apply({type:'set_draft',path:'config.backend.args',value:'[]'});apply({type:'set_draft',path:'config.backend.env',value:'{}'});apply({type:'set_draft',path:'config.backend.working_directory',value:'${AGENT_HOME}/workspace'});apply({type:'navigate',route:'setup',step:3});await page.reload();await page.locator('[data-action="backend.account.connect"]').waitFor();assert.equal(view.state.step,1);assert.equal(view.state.draft.config?.backend,undefined);assert.equal(view.state.draft.config?.name,'Atlas');
 // Meaningful custom edits must survive the upgrade migration.
 apply({type:'set_draft',path:'config.backend.command',value:'/custom/acp-runtime'});await page.reload();await page.locator('[data-action="setup.finish"]').waitFor();assert.equal(view.state.draft.config?.backend.command,'/custom/acp-runtime');
 // Clear only that test edit; preserve identity drafts, then simulate a missing installation.
 delete view.state.draft.config.backend;config.backend=null;runtimeInstalled=false;revision++;await page.reload();await page.locator('[data-action="backend.install"]').waitFor();assert(await page.locator('[data-action="setup.finish"]').isDisabled());assert.equal(await page.locator('[data-draft="config.backend.command"]').count(),0);await page.locator('[data-action="backend.install"]').click();await page.locator('[data-action="backend.account.connect"]').waitFor();assert(runtimeInstalled&&config.backend);assert.equal(view.state.draft.config.name,'Atlas');
 await page.screenshot({path:path.join(artifacts,'setup-account.png'),fullPage:true});
 await page.locator('[data-action="backend.account.connect"]').click();await page.locator('[data-control="account.pending"]').waitFor();assert((await page.locator('[data-control="account.pending"]').innerText()).includes('FIXTURE-CODE'));assert.equal(await page.locator('[data-control="account.open"]').getAttribute('href'),'https://auth.openai.com/codex/device');
 await page.waitForFunction(()=>!document.querySelector('[data-action="setup.finish"]')?.disabled);await page.locator('[data-action="setup.finish"]').click();
 await page.waitForFunction(()=>document.querySelector('h1')?.textContent==='A little clarity, at a glance.');assert.equal(config.name,'Atlas');assert(config.setup_complete&&config.enabled);assert.equal(config.backend.command,'/agent/runtime/bin/codex-acp');assert.equal(config.server.welcome_port,41412);assert((await page.locator('.timeline').innerText()).includes('Completed a request'));assert(!(await page.locator('.timeline').innerText()).includes('Display update'));
 await page.screenshot({path:path.join(artifacts, 'dashboard.png'),fullPage:true});
 // An external API controller drives the visible route, draft and selected session.
 apply({type:'navigate',route:'configuration'});await page.locator('[data-draft="config.instructions"]').waitFor();assert.equal(await page.locator('[data-draft="config.backend.command"]').count(),0);assert.equal(await page.locator('[data-draft="readonly.owner"]').count(),0);
 apply({type:'set_filter',key:'advanced',value:true});await page.locator('[data-draft="config.backend.command"]').waitFor();assert.equal(await page.locator('[data-action="advanced.toggle"]').getAttribute('aria-expanded'),'true');await page.locator('[data-draft="config.tool_connections"]').fill(JSON.stringify([{name:'project-tools',url:'https://tools.example/mcp',header_secrets:{Authorization:'project-auth'}}]));assert.equal(await page.locator('[data-draft="config.server.welcome_port"]').inputValue(),'41412');assert.equal(await page.locator('[data-draft="config.server.pam_service"]').count(),process.env.UI_TEST_AUTH_METHOD==='windows_logon'?0:1);assert((await page.locator('.advanced-content').innerText()).includes('http://127.0.0.1:41412'));assert(!(await page.locator('.advanced-content').innerText()).includes('http://localhost:41412'));await page.screenshot({path:path.join(artifacts,'advanced.png'),fullPage:true});await page.locator('[data-action="advanced.toggle"]').click();await page.locator('[data-draft="config.backend.command"]').waitFor({state:'hidden'});assert.equal(view.state.filters.advanced,false);apply({type:'set_draft',path:'config.instructions',value:'Remote controller wrote this draft.'});await page.waitForFunction(()=>document.querySelector('[data-draft="config.instructions"]')?.value==='Remote controller wrote this draft.');
 await page.locator('[data-action="config.save"]').first().click();await page.waitForFunction(()=>document.querySelector('.banner')?.textContent.includes('Configuration saved'));assert.equal(config.instructions,'Remote controller wrote this draft.');assert.equal(config.tool_connections[0].header_secrets.Authorization,'project-auth');assert.equal(config.enabled,true);
 apply({type:'navigate',route:'connections'});await page.locator('[data-control="context.sources"]').waitFor();assert((await page.locator('[data-control="context.sources"]').innerText()).includes('Workshop Slack'));assert((await page.locator('[data-control="context.sources"]').innerText()).includes('Credential needed'));apply({type:'select_session',session_id:'session-one'});await page.locator('[data-control="session.detail"]').waitFor();await page.waitForFunction(()=>document.querySelector('[data-control="session.detail"]')?.textContent.includes('example'));
 apply({type:'navigate',route:'activity'});await page.locator('[data-control="activity.results"]').waitFor();await page.waitForFunction(()=>document.querySelector('.activity-list')?.textContent.includes('Completed a request'));assert.equal(await page.locator('.activity-entry').count(),3);assert(!(await page.locator('[data-control="activity.results"]').innerText()).includes('Display'));
 await page.screenshot({path:path.join(artifacts,'activity.png'),fullPage:true});
 await page.locator('[data-control="activity.problems"]').check();await page.waitForFunction(()=>document.querySelectorAll('.activity-entry').length===1);assert((await page.locator('.activity-entry').innerText()).includes('needs attention'));assert.equal(view.state.filters.problems_only,true);
 apply({type:'set_filter',key:'problems_only',value:false});apply({type:'set_filter',key:'category',value:'outcomes'});await page.waitForFunction(()=>document.querySelectorAll('.activity-entry').length===1&&document.querySelector('.activity-entry')?.textContent.includes('Completed'));assert.equal(await page.locator('[data-control="activity.category"]').inputValue(),'outcomes');
 apply({type:'set_filter',key:'category',value:'all'});apply({type:'set_filter',key:'search',value:'Completed'});await page.waitForFunction(()=>document.querySelectorAll('.activity-entry').length===1&&document.querySelector('[data-control="activity.search"]')?.value==='Completed');
 apply({type:'set_filter',key:'search',value:''});apply({type:'set_filter',key:'diagnostics',value:true});await page.waitForFunction(()=>document.querySelector('.activity-list')?.textContent.includes('Display'));assert(await page.locator('[data-control="activity.diagnostics"]').isChecked());await page.locator('[data-action="activity.more"]').click();await page.waitForFunction(()=>document.querySelectorAll('.activity-entry').length===200);assert(requests.some(r=>r.path.endsWith('/activity')&&r.query.includes('before=')));
 apply({type:'set_filter',key:'diagnostics',value:false});await page.waitForFunction(()=>document.querySelectorAll('.activity-entry').length===3);assert(!(await page.locator('[data-control="activity.results"]').innerText()).includes('Display'));
 await page.locator('[data-route="profiles"]').click();await page.waitForFunction(()=>document.querySelector('[data-control="profile.use"]')?.textContent.includes('No persona profile configured'));assert.equal(await page.locator('[data-control="profile.use"] .pill.green').count(),0);await page.locator('[data-draft="profiles.communication"]').fill('Thoughtful, direct, and concise.');await page.locator('[data-action="profile.save"][data-profile="communication"]').click();await page.waitForFunction(()=>document.querySelector('.banner')?.textContent.includes('Communication profile saved'));assert.equal(profiles.communication,'Thoughtful, direct, and concise.');
 const originalProfile=profilePackage.active.id;
 await page.locator('[data-draft="profiles.identity"]').fill('Private identity stays separate.');await page.locator('[data-action="profile.save"][data-profile="identity"]').click();await page.waitForFunction(()=>document.querySelector('.banner')?.textContent.includes('Identity & personal context profile saved'));
 const imported=fixturePackage('source-version-one','Imported voice unique marker.','Imported working guidance unique marker.');
 await page.locator('[data-action="profile.package.open"]').click();await page.locator('[data-control="profile.package.file"]').setInputFiles({name:'invalid.json',mimeType:'application/json',buffer:Buffer.from('{not JSON')});await page.waitForFunction(()=>document.querySelector('.modal .banner.error')?.textContent.includes('not valid JSON'));assert(await page.locator('[data-control="profile.package.apply"]').isDisabled());
 await page.locator('[data-control="profile.package.file"]').setInputFiles({name:'alex.twin.json',mimeType:'application/json',buffer:Buffer.from(JSON.stringify(imported))});await page.locator('[data-control="profile.package.preview"]').waitFor();assert(!(JSON.stringify(view).includes('private-import-evidence-marker')));await page.locator('[data-control="profile.package.apply"]').click();await page.waitForFunction(()=>document.querySelector('.banner')?.textContent.includes('Profile installed'));assert.equal(profiles.communication,imported.files['communication.md']);assert.equal(profiles['working-style'],imported.files['working-style.md']);assert.equal(profiles.identity,'Private identity stays separate.');assert.equal(await page.locator('[data-draft="profiles.communication"]').inputValue(),imported.files['communication.md']);
 apply({type:'set_filter',key:'profile_package_history',value:true});await page.locator('[data-control="profile.package.versions"]').waitFor();assert((await page.locator('[data-control="profile.package.status"]').innerText()).includes('source-version-one'));await page.screenshot({path:path.join(artifacts,'profile-package.png'),fullPage:true});
 await page.setViewportSize({width:390,height:844});await page.screenshot({path:path.join(artifacts,'profile-package-mobile.png'),fullPage:true});assert(await page.evaluate(()=>document.documentElement.scrollWidth<=window.innerWidth+1));await page.setViewportSize({width:1440,height:1050});
 // A reviewed version cannot silently overwrite a later update by another manager.
 const secondPackage=fixturePackage('source-version-two','Second imported voice.','Second imported working style.');
 await page.evaluate(pkg=>window.agentInstanceControl.perform('profile.package.prepare',{package:pkg}),secondPackage);profiles.communication='A newer external edit.';saveProfileVersion();await page.locator('[data-control="profile.package.apply"]').click();await page.waitForFunction(()=>document.querySelector('.modal .banner.error')?.textContent.includes('changed while you were reviewing'));assert.equal(profiles.communication,'A newer external edit.');await page.locator('[data-control="profile.package.apply"]').click();await page.waitForFunction(()=>!document.querySelector('.modal'));assert.equal(profiles.communication,'Second imported voice.');
 // Restore respects local unsaved edits; discarding them makes the previous version available.
 await page.locator('[data-draft="profiles.communication"]').fill('Keep my pending voice edit.');const blocked=await page.evaluate(async id=>{try{await window.agentInstanceControl.perform('profile.package.restore',{id});return null;}catch(e){return e.message;}},originalProfile);assert(blocked.includes('Save or discard'));assert.equal(profiles.communication,'Second imported voice.');assert.equal(await page.locator('[data-draft="profiles.communication"]').inputValue(),'Keep my pending voice edit.');await page.locator('[data-action="profile.discard"][data-profile="communication"]').click();await page.waitForFunction(()=>document.querySelector('[data-draft="profiles.communication"]')?.value==='Second imported voice.');await page.locator(`[data-action="profile.package.restore"][data-id="${originalProfile}"]`).click();await page.waitForFunction(()=>document.querySelector('.banner')?.textContent.includes('Previous profile restored'));assert.equal(profiles.communication,'Thoughtful, direct, and concise.');assert.equal(profiles['working-style'],'');assert.equal(profiles.identity,'Private identity stays separate.');
 // Domain parity: importing does not require opening a native file picker or a dialog.
 const programmatic=await page.evaluate(({pkg,revision})=>window.agentInstanceControl.perform('profile.package.import',{package:pkg,expected_revision:revision}),{pkg:imported,revision:profilePackage.revision});assert.equal(programmatic.active.source.revision,'source-version-one');assert.equal(profiles.identity,'Private identity stays separate.');assert(!JSON.stringify(view).includes('Imported voice unique marker.'));assert(!JSON.stringify(view).includes('private-import-evidence-marker'));
 // A controller can edit the displayed draft while an installation is in flight.
 // Installing must not erase those newer edits after its response arrives.
 let resumeApply,enteredApply;const arrived=new Promise(resolve=>enteredApply=resolve),resume=new Promise(resolve=>resumeApply=resolve);delayedProfileApply={entered:enteredApply,resume};
 const installing=page.evaluate(({pkg,revision})=>window.agentInstanceControl.perform('profile.package.import',{package:pkg,expected_revision:revision}),{pkg:secondPackage,revision:profilePackage.revision});await arrived;await page.evaluate(()=>window.agentInstanceControl.setDraft('profiles.communication','Draft entered during profile installation.'));resumeApply();await installing;assert.equal(profiles.communication,'Second imported voice.');assert.equal(await page.locator('[data-draft="profiles.communication"]').inputValue(),'Draft entered during profile installation.');await page.evaluate(()=>window.agentInstanceControl.perform('profile.discard',{profile:'communication'}));
 await page.locator('[data-route="access"]').click();await page.waitForFunction(()=>document.querySelector('h1')?.textContent==='Access, kept intentional.');await page.locator('.detail-list').waitFor();assert((await page.locator('.detail-list').innerText()).includes(process.env.UI_TEST_AUTH_METHOD==='windows_logon'?'Windows account':'PAM'));await page.locator('[data-action="secret.new"]').click();await page.locator('[data-draft="access.secret_name"]').fill('provider-key');await page.locator('[data-secret-input]').fill('fixture-secret-value');await page.locator('[data-action="secret.save"]').click();await page.waitForFunction(()=>document.querySelector('.banner')?.textContent.includes('stored'));assert.equal(secrets['provider-key'],'fixture-secret-value');
 await page.locator('[data-action="token.new"]').click();await page.locator('[data-draft="access.token_name"]').fill('My client');await page.locator('[data-draft="access.token_scopes.profiles"]').check();await page.locator('[data-draft="access.token_scopes.tools:use"]').check();await page.locator('[data-action="token.create"]').click();await page.locator('[data-token-once]').waitFor();assert.equal(await page.locator('[data-token-once]').inputValue(),'fixture-one-time-token');assert(tokens[0].scopes.includes('profiles'));assert(tokens[0].scopes.includes('tools:use'));await page.locator('[data-action="modal.close"]').last().click();
 const telemetry=JSON.stringify(requests.filter(r=>r.path.includes('/views')));assert(!telemetry.includes('fixture-secret-value'));assert(!telemetry.includes('fixture-login-password'));assert(!telemetry.includes('fixture-one-time-token'));assert(!telemetry.includes('FIXTURE-CODE'));assert(!telemetry.includes('fixture-login'));assert(requests.some(r=>r.path.endsWith('/backend/install')&&r.method==='POST'));assert(requests.some(r=>r.path.endsWith('/activity')&&r.query.includes('category=problems')));assert(!JSON.stringify(view).includes('fixture-secret-value'));assert(view.ack);
 assert(!telemetry.includes('private-import-evidence-marker'));assert(!telemetry.includes('Imported voice unique marker.'));assert(!telemetry.includes('Imported working guidance unique marker.'));
 }
 // Production client wrappers must preserve newer intent, even when a conflict arrives mid-command.
 apply({type:'navigate',route:'configuration'});apply({type:'set_draft',path:'config.purpose',value:'Original purpose'});
 await page.waitForFunction(revision=>window.agentInstanceControl.getState().revision===revision,view.revision);
 const firstWrite=requests.length;
 beforeViewWrite=()=>apply({type:'set_draft',path:'config.purpose',value:'Newer remote purpose'});
 const staleCommand=await page.evaluate(async()=>{try{await window.agentInstanceControl.setDraft('config.purpose','Older queued purpose');return null;}catch(e){return e.message;}});
 assert(staleCommand,'Conflicting old command must report the conflict');
 assert.equal(view.state.draft.config.purpose,'Newer remote purpose','A stale command must not overwrite newer same-field intent');
 assert.equal(requests.slice(firstWrite).filter(r=>r.body?.command?.value==='Older queued purpose').length,1,'Conflicting command must not be reposted');
 // Unrelated newer changes are compatible with the original command's affected fields.
 beforeViewWrite=()=>apply({type:'set_filter',key:'unrelated_fixture',value:'Keep this newer field'});
 await page.evaluate(()=>window.agentInstanceControl.setDraft('config.purpose','Compatible purpose'));
 assert.equal(view.state.draft.config.purpose,'Compatible purpose');assert.equal(view.state.filters.unrelated_fixture,'Keep this newer field');
 // Focus is protected just like draft text; a fresh revision is not fresh intent.
 beforeViewWrite=()=>apply({type:'focus',target:'config.instructions'});
 const staleFocus=await page.evaluate(async()=>{try{await window.agentInstanceControl.command({type:'focus',target:'config.purpose'});return null;}catch(e){return e.message;}});
 assert(staleFocus);assert.equal(view.state.focus,'config.instructions');
 // patchView must not rerun an unguarded namespace cleanup on newer state.
 beforeViewWrite=()=>apply({type:'set_draft',path:'config.instructions',value:'New draft during cleanup'});
 const staleCleanup=await page.evaluate(async()=>{try{await window.agentInstanceControl.perform('draft.discard');return null;}catch(e){return e.message;}});
 assert(staleCleanup,'A stale cleanup must report conflict');assert.equal(view.state.draft.config.instructions,'New draft during cleanup');
 // A command waiting behind another local request retains its original observed intent.
 let releaseViewWrite,enteredViewWrite;
 const heldWrite=new Promise(resolve=>enteredViewWrite=resolve),releaseWrite=new Promise(resolve=>releaseViewWrite=resolve);
 beforeViewWrite=async()=>{enteredViewWrite();await releaseWrite;};
 const waiting=page.evaluate(()=>{
  const first=window.agentInstanceControl.command({type:'set_filter',key:'queue_fixture',value:true});
  const second=window.agentInstanceControl.setDraft('config.purpose','Old waiting purpose');
  return Promise.allSettled([first,second]).then(rows=>rows.map(r=>({status:r.status,error:r.reason?.message})));
 });
 await heldWrite;apply({type:'set_draft',path:'config.purpose',value:'Newer purpose before queued dispatch'});
 await page.waitForFunction(revision=>window.agentInstanceControl.getState().revision===revision,view.revision);
 releaseViewWrite();const queueResults=await waiting;
 assert.equal(queueResults[0].status,'fulfilled','Unrelated queued filter can reconcile');assert.equal(queueResults[1].status,'rejected','Queued intent must not acquire a fresh base silently');
 assert.equal(view.state.draft.config.purpose,'Newer purpose before queued dispatch');
 // Unrelated local fields may share a newly created draft namespace.
 await page.evaluate(()=>Promise.all([window.agentInstanceControl.setDraft('access.secret_name','A reference name'),window.agentInstanceControl.setDraft('access.token_name','A token label')]));
 assert.equal(view.state.draft.access.secret_name,'A reference name');assert.equal(view.state.draft.access.token_name,'A token label');
 // Local edits keep their own causal order while still rejecting outside changes.
 let releaseLocal,enteredLocal;const localEntered=new Promise(resolve=>enteredLocal=resolve),localRelease=new Promise(resolve=>releaseLocal=resolve);
 beforeViewWrite=async()=>{enteredLocal();await localRelease;};
 const localWrites=page.evaluate(()=>Promise.all([window.agentInstanceControl.setDraft('config.purpose','Local first'),window.agentInstanceControl.setDraft('config.purpose','Local second')]));
 await localEntered;releaseLocal();await localWrites;assert.equal(view.state.draft.config.purpose,'Local second');
 // Real typing can queue a newer value while its earlier save is in flight.
 await page.evaluate(()=>window.agentInstanceControl.command({type:'focus',target:'config.purpose'}));
 let releaseTyped,enteredTyped;const typedEntered=new Promise(resolve=>enteredTyped=resolve),typedRelease=new Promise(resolve=>releaseTyped=resolve);
 beforeViewWrite=async()=>{enteredTyped();await typedRelease;};beforeViewWrite.matches=data=>data.command?.type==='set_draft'&&data.command.path==='config.purpose';
 await page.locator('[data-draft="config.purpose"]').fill('Typed first');await typedEntered;
 await page.locator('[data-draft="config.purpose"]').fill('Typed second');releaseTyped();
 await page.waitForFunction(()=>window.agentInstanceControl.getState().state.draft.config.purpose==='Typed second');assert.equal(view.state.draft.config.purpose,'Typed second');
 // A same-route selection/revision change cannot redirect a delayed draft to other work.
 await page.evaluate(()=>window.agentInstanceControl.command({type:'set_filter',key:'work_id',value:'work-A'}));
 beforeViewWrite=()=>apply({type:'set_filter',key:'work_id',value:'work-B'});
 const changedWork=await page.evaluate(async()=>{try{await window.agentInstanceControl.setDraft('work.goal','Draft for work A');return null;}catch(e){return e.message;}});
 assert(changedWork);assert.equal(view.state.filters.work_id,'work-B');assert.equal(view.state.draft.work?.goal,undefined);
 await page.evaluate(()=>window.agentInstanceControl.command({type:'set_filter',key:'work_revision',value:1}));
 beforeViewWrite=()=>apply({type:'set_filter',key:'work_revision',value:2});
 const changedRevision=await page.evaluate(async()=>{try{await window.agentInstanceControl.setDraft('work.goal','Draft for revision 1');return null;}catch(e){return e.message;}});
 assert(changedRevision);assert.equal(view.state.filters.work_revision,2);assert.equal(view.state.draft.work?.goal,undefined);
 // A delayed typed draft remains local after conflict, without replacing the new shared value.
 await page.locator('[data-draft="config.purpose"]').fill('Unsaved local purpose');
 apply({type:'set_draft',path:'config.purpose',value:'Newest shared purpose'});
 await page.waitForFunction(revision=>window.agentInstanceControl.getState().revision===revision,view.revision);
 await page.waitForTimeout(650);
 assert.equal(view.state.draft.config.purpose,'Newest shared purpose');
 assert.equal(await page.locator('[data-draft="config.purpose"]').inputValue(),'Unsaved local purpose');
 assert((await page.locator('[role="alert"]').allTextContents()).some(text=>text.includes('changed')),'Conflict must leave a visible explanation');
 // Discard is a new explicit intent, and clears retained local drafts only after its view write succeeds.
 await page.evaluate(()=>window.agentInstanceControl.perform('draft.discard'));
 // Keep the existing expected-text profile cleanup: a new draft during package install wins.
 apply({type:'navigate',route:'profiles'});await page.locator('[data-draft="profiles.communication"]').waitFor();
 let releaseProfile,enteredProfile;const profileEntered=new Promise(resolve=>enteredProfile=resolve),profileRelease=new Promise(resolve=>releaseProfile=resolve);
 delayedProfileApply={entered:enteredProfile,resume:profileRelease};
 const packageRun=page.evaluate(({pkg,revision})=>window.agentInstanceControl.perform('profile.package.import',{package:pkg,expected_revision:revision}),{pkg:fixturePackage('intent-fixture','Installed profile text.','Working text.'),revision:profilePackage.revision});
 await profileEntered;await page.evaluate(()=>window.agentInstanceControl.setDraft('profiles.communication','New draft during package install'));releaseProfile();await packageRun;
 assert.equal(profiles.communication,'Installed profile text.');assert.equal(view.state.draft.profiles.communication,'New draft during package install');
 await page.evaluate(()=>window.agentInstanceControl.perform('profile.discard',{profile:'communication'}));
 // Ordinary profile save uses the same expected-text cleanup guard as package install.
 await page.evaluate(()=>window.agentInstanceControl.setDraft('profiles.communication','Text being saved'));
 let releaseSave,enteredSave;const saveEntered=new Promise(resolve=>enteredSave=resolve),saveRelease=new Promise(resolve=>releaseSave=resolve);
 delayedProfileWrite={entered:enteredSave,resume:saveRelease};
 const savingProfile=page.evaluate(()=>window.agentInstanceControl.perform('profile.save',{profile:'communication'}));
 await saveEntered;await page.evaluate(()=>window.agentInstanceControl.setDraft('profiles.communication','New text while saving'));releaseSave();await savingProfile;
 assert.equal(profiles.communication,'Text being saved');assert.equal(view.state.draft.profiles.communication,'New text while saving');
 apply({type:'navigate',route:'access'});await page.waitForFunction(()=>window.agentInstanceControl.getState().state.route==='access');
 // Saving a secret consumes its non-secret name draft before leaving that modal.
 await page.evaluate(()=>window.agentInstanceControl.perform('secret.new'));
 await page.locator('[data-draft="access.secret_name"]').fill('intent-fixture-secret');
 await page.locator('[data-secret-input]').fill('fictional-secret-only-in-fixture');
 await page.evaluate(()=>window.agentInstanceControl.perform('secret.save'));
 assert.equal(secrets['intent-fixture-secret'],'fictional-secret-only-in-fixture');
 // Debounce and a submit action can flush the very same draft while its first write waits.
 await page.evaluate(()=>window.agentInstanceControl.perform('token.new'));
 let releaseDuplicate,enteredDuplicate;const duplicateEntered=new Promise(resolve=>enteredDuplicate=resolve),duplicateRelease=new Promise(resolve=>releaseDuplicate=resolve);
 beforeViewWrite=async()=>{enteredDuplicate();await duplicateRelease;};beforeViewWrite.matches=data=>data.command?.type==='set_draft'&&data.command.path==='access.token_name';
 await page.locator('[data-draft="access.token_name"]').fill('One pending flush');await duplicateEntered;
 const creatingToken=page.evaluate(()=>window.agentInstanceControl.perform('token.create'));
 await page.waitForTimeout(40);releaseDuplicate();await creatingToken;
 assert.equal(tokens.at(-1).name,'One pending flush');assert.equal(requests.filter(r=>r.body?.command?.value==='One pending flush').length,1);await page.locator('[data-token-once]').waitFor();
 await page.evaluate(()=>window.agentInstanceControl.perform('modal.close'));
 // Untrusted postMessage cannot puppeteer the page.
 await page.evaluate(()=>window.postMessage({protocol:'agent-instance.ui/1',id:'untrusted',type:'command',command:{type:'navigate',route:'setup'}},location.origin));await page.waitForTimeout(250);assert.equal(view.state.route,'access');
 await page.setViewportSize({width:390,height:844});await page.screenshot({path:path.join(artifacts, 'mobile.png'),fullPage:true});assert(await page.evaluate(()=>document.documentElement.scrollWidth<=window.innerWidth+1));
 assert(requests.every(r=>r.path.startsWith(prefix)));assert.equal(errors.length,0,JSON.stringify(errors));console.log(JSON.stringify({passed:true,checks:viewIntentOnly?['production-wrapper same-field conflict without replay','unrelated-field reconciliation','namespace cleanup preserves newer drafts','queued intent keeps original base','delayed typed draft conflict preserves local text','newer focus retained','guarded profile cleanup retains newer draft','local queued same-field edits retain order','typing during in-flight draft save retains order','changed work selection and revision refuse stale draft','profile save retains newer draft','new draft namespace preserves separate local fields','debounce and submit coalesce the same draft flush','secret-name draft settles before opening token form','no browser exceptions']:['production-wrapper same-field conflict without replay','unrelated-field reconciliation','namespace cleanup preserves newer drafts','queued intent keeps original base','delayed typed draft conflict preserves local text','prefix assets and API paths','account login flow against fixture','platform-appropriate sign-in description and settings','two-step default setup without technical fields','explicit account readiness and device sign-in','older setup-step and empty-runtime-draft migration with identity edits preserved','nonempty custom runtime draft preserved','missing runtime shown with one install action','optional advanced settings controlled locally and remotely','server-filtered meaningful activity despite 1200 newer diagnostics','category, search, problems and diagnostics filters','activity pagination','setup and runtime start','remote route and draft via SSE','configuration save','advanced connected-tool references and explicit use grant','remote session selection','empty persona reports not configured without a supplied badge','profile save','portable profile upload with malformed-file recovery','profile import and restore preserve separate identity','stale profile update requires explicit retry','restore protects unsaved profile edits','profile API actions work without native file picker','imported profiles and evidence absent from view telemetry','profiles-scoped token creation','profile version history remotely controlled and mobile layout','write-only secret','one-time token','no credentials in view telemetry','render acknowledgement','untrusted embedding rejected','mobile overflow','no browser exceptions'],screenshots:viewIntentOnly?[path.join(artifacts,'mobile.png')]:[path.join(artifacts, 'dashboard.png'),path.join(artifacts, 'mobile.png')]}));
} catch(error) {console.error(JSON.stringify({errors,missingRoutes,view,recentRequests:requests.slice(-10)},null,2));await page.screenshot({path:path.join(artifacts,"failure.png"),fullPage:true});throw error;} finally {await browser.close();for(const r of streams)r.end();server.close();}
