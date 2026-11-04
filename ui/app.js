(() => {
  "use strict";

  const root = document.getElementById("app");
  const base = new URL("./", document.baseURI);
  const API = new URL("api/v1/", base);
  const routes = ["dashboard", "work", "memory", "organization", "configuration", "profiles", "connections", "activity", "access"];
  const titles = {memory:"Memory",work:"Responsibilities",organization:"Roles & relationships",setup:"Set up your agent",dashboard:"Overview",configuration:"Configuration",profiles:"Context & profiles",connections:"Connections",activity:"Activity",access:"Access & credentials"};
  const profileNames = {communication:"Communication", "working-style":"Working style", identity:"Identity & personal context"};
  const icons = {
    overview:'<rect x="3" y="3" width="7" height="7" rx="1.5"/><rect x="14" y="3" width="7" height="7" rx="1.5"/><rect x="3" y="14" width="7" height="7" rx="1.5"/><rect x="14" y="14" width="7" height="7" rx="1.5"/>',
    settings:'<path d="M4 7h16M4 17h16"/><circle cx="9" cy="7" r="3"/><circle cx="15" cy="17" r="3"/>',
    profile:'<path d="M5 4h9l5 5v11H5zM14 4v5h5M8 13h8M8 16h6"/>',
    connections:'<rect x="2" y="8" width="6" height="8" rx="2"/><rect x="16" y="8" width="6" height="8" rx="2"/><path d="M8 12h8M12 8v8"/>',
    activity:'<path d="M3 12h4l3-7 4 14 3-7h4"/>',
    access:'<rect x="5" y="10" width="14" height="11" rx="2"/><path d="M8 10V6a4 4 0 0 1 8 0v4M12 14v3"/>',
    arrow:'<path d="M5 12h14M14 7l5 5-5 5"/>',
    check:'<path d="m5 12 4 4L19 6"/>',
    close:'<path d="m6 6 12 12M18 6 6 18"/>',
    clock:'<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/>',
    copy:'<rect x="8" y="8" width="12" height="13" rx="2"/><path d="M16 8V3H3v13h5"/>',
    plus:'<path d="M12 5v14M5 12h14"/>',
    play:'<path d="m8 5 11 7-11 7z"/>',
    stop:'<rect x="6" y="6" width="12" height="12" rx="1"/>',
    refresh:'<path d="M20 8a8 8 0 1 0 0 8M20 3v5h-5"/>',
    info:'<circle cx="12" cy="12" r="9"/><path d="M12 11v6M12 7v.5"/>',
    terminal:'<path d="m5 6 6 6-6 6M13 18h6"/>',
    screen:'<rect x="3" y="4" width="18" height="13" rx="2"/><path d="M8 21h8M12 17v4"/>',
    chevron:'<path d="m9 5 7 7-7 7"/>'
  };
  const app = {
    development:null,developmentError:"",nativeConfig:null,nativeSchema:null,nativeError:"",nativeAccount:null,nativeAccountError:"",nativeAccountTimer:null,nativeModels:null,memory:null,memoryError:"",scheduler:null,execution:null,schedulerError:"",work:null,organization:null,workError:"",profileUse:null,profileUseError:"",workSaving:false,
    contextProviders:[], contextError:"", session:null, csrf:"", doc:null, status:null, schema:null, view:null, events:[], connections:[], sessions:[],
    secrets:[], tokens:[], profiles:{}, profilePackage:null, profilePackageError:"", profilePackageBusy:false, profilePackageImport:null, profilePackageImportError:"", profilePackageExpected:null, sessionDetail:null, source:null, online:true, streamOnline:false,
    error:"", notice:"", busy:false, tokenOnce:null, draftPending:new Map(), timers:new Map(),
    commandQueue:Promise.resolve(), lastAck:0, initialScroll:true, poll:null, refreshTimer:null,
    focusRemote:null, scrollRemote:null, loading:true, latestEvent:0, renderError:null,
    backend:null, backendBusy:false, account:null, accountTimer:null, activity:[], activityRecent:[],
    activityNext:null, activityHasMore:false, activityLoading:false, activityError:"", recentActivityError:"", activityRequest:0, activityQuery:""
  };

  function icon(name) { return `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.65" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${icons[name] || icons.info}</svg>`; }
  function esc(v) { return String(v ?? "").replace(/[&<>"']/g, c => ({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"}[c])); }
  function clone(v) { return JSON.parse(JSON.stringify(v)); }
  function asArray(v) { return Array.isArray(v) ? v : (v?.items || v?.records || []); }
  function title(v) { return String(v || "").replace(/[._-]/g," ").replace(/\b\w/g,c=>c.toUpperCase()); }
  function shortId(v) { const s=String(v||""); return s.length>19 ? `${s.slice(0,8)}…${s.slice(-6)}` : s; }
  function initial() { return (app.doc?.config.name || "Agent").trim().slice(0,2).toUpperCase(); }
  function get(object,path) { return path.split(".").reduce((v,k)=>v?.[k],object); }
  function put(object,path,value) { const bits=path.split("."); let o=object; for(const k of bits.slice(0,-1)){if(!o[k] || typeof o[k]!=="object")o[k]={};o=o[k];}o[bits.at(-1)]=value; }
  function state() { return app.view?.state || {route:app.doc?.config.setup_complete ? "dashboard":"setup",step:0,filters:{},draft:{}}; }
  function currentRoute() { return titles[state().route] ? state().route : "dashboard"; }
  function draft(key,fallback="") { if(app.draftPending.has(key))return app.draftPending.get(key); const d=state().draft||{}; return Object.hasOwn(d,key) ? d[key] : (get(d,key) ?? fallback); }
  function displayTime(value) { if(!value)return "—"; const n=typeof value==="number" && value<100000000000 ? value*1000 : value; const d=new Date(n); return isNaN(d.getTime()) ? String(value) : d.toLocaleString([], {month:"short",day:"numeric",hour:"numeric",minute:"2-digit"}); }
  function age(ms) { const n=Math.max(0,Number(ms)||0); if(n<60000)return "Just started"; if(n<3600000)return `${Math.floor(n/60000)} min`; if(n<86400000)return `${Math.floor(n/3600000)}h ${Math.floor(n%3600000/60000)}m`; return `${Math.floor(n/86400000)}d ${Math.floor(n%86400000/3600000)}h`; }
  function eventTime(e) { return e.at ?? e.time ?? e.timestamp; }
  function eventSummary(e) { return e.summary || e.data?.message || e.data?.detail || e.data?.reason || (e.data?.session_id ? `Session ${shortId(e.data.session_id)}` : ""); }
  function announce(s) { document.getElementById("announcements").textContent=s; }
  function setNotice(s) { app.notice=s; app.error=""; announce(s); render(); }
  function fail(error) { app.error=error.message || String(error); announce(app.error); render(); }
  function listIcon(route) { return ({memory:"profile",work:"profile",organization:"connections",dashboard:"overview",configuration:"settings",profiles:"profile",connections:"connections",activity:"activity",access:"access",setup:"plus"})[route]; }
  function safeLink(url) { try { const u=new URL(url,base); return ["https:","http:"].includes(u.protocol) ? u.href : ""; } catch {return "";} }

  async function api(path,options={}) {
    const method=options.method || "GET";
    const headers={Accept:"application/json",...(options.headers||{})};
    if(options.body!==undefined)headers["Content-Type"]="application/json";
    if(method!=="GET" && app.csrf)headers["X-CSRF-Token"]=app.csrf;
    const response=await fetch(new URL(path,API),{...options,method,headers,credentials:"same-origin",cache:"no-store",body:options.body===undefined ? undefined : JSON.stringify(options.body)});
    const raw=await response.text(); let data; try {data=raw?JSON.parse(raw):null;}catch{data={error:raw?"The service returned an unexpected response.":"Empty response."};}
    if(!response.ok){const e=new Error(typeof data?.error==="string" ? data.error : data?.error?.message || data?.message || `Request failed (${response.status}).`);e.status=response.status;e.data=data;if(response.status===401 && !path.startsWith("auth/")){signedOut();}throw e;}
    return data;
  }

  function button(label,action,opts={}) { return `<button class="button ${opts.primary?"primary":""} ${opts.danger?"danger":""} ${opts.quiet?"quiet":""} ${opts.small?"small":""}" type="button" data-action="${esc(action)}" ${opts.attrs||""} ${opts.disabled||app.busy?"disabled":""}>${opts.icon?icon(opts.icon):""}${esc(label)}</button>`; }
  function badge(label,kind="") { return `<span class="pill ${kind}">${kind==="green"?'<span class="status-dot"></span>':""}${esc(label)}</span>`; }
  function empty(heading,text,ico="connections") { return `<div class="empty">${icon(ico)}<strong>${esc(heading)}</strong><span>${esc(text)}</span></div>`; }
  function banner(message,kind="") { return `<div class="banner ${kind}" role="${kind==="error"?"alert":"status"}">${icon("info")}<div>${esc(message)}</div></div>`; }
  function heading(label,copy,actions="") { return `<div class="page-heading"><div><div class="eyebrow">Your agent, independently</div><h1>${esc(label)}</h1><p>${esc(copy)}</p></div>${actions?`<div class="heading-actions">${actions}</div>`:""}</div>`; }
  function field(label,key,value,opts={}) {
    const id=key.replace(/[^a-zA-Z0-9_-]/g,"-");
    const raw=draft(key,value); const v=raw && typeof raw==="object" ? JSON.stringify(raw,null,2) : raw; const attrs=`id="${id}" data-action="edit-draft" data-draft="${esc(key)}" data-control="${esc(key)}" ${opts.readonly?"readonly":""} ${opts.required?"required":""}`;
    const control=opts.textarea ? `<textarea class="input ${opts.code?"code":""} ${opts.long?"long":""}" ${attrs} rows="${opts.rows||4}" spellcheck="${opts.code?"false":"true"}">${esc(v)}</textarea>` : `<input class="input ${opts.code?"mono":""}" ${attrs} type="${opts.type||"text"}" value="${esc(v)}" ${opts.min!==undefined?`min="${opts.min}"`:""} ${opts.max!==undefined?`max="${opts.max}"`:""} placeholder="${esc(opts.placeholder||"")}" autocomplete="off">`;
    return `<label class="field ${opts.full?"full":""}" for="${id}"><span class="field-label">${esc(label)}${opts.readonly?'<span class="readonly-badge">Read only</span>':""}</span>${control}${opts.help?`<p class="field-help">${esc(opts.help)}</p>`:""}</label>`;
  }
  function configField(label,path,opts={}) {
    let value=get(app.doc.config,path); if(opts.json)value=JSON.stringify(value ?? opts.default ?? (opts.array?[]:{}),null,2);else value=value??opts.default??"";
    return field(label,`config.${path}`,value,{...opts,textarea:opts.json || opts.textarea,code:opts.json||opts.code});
  }
  function checkbox(label,key,fallback,help="") { return `<label class="check"><input type="checkbox" data-action="edit-draft" data-draft="${esc(key)}" data-control="${esc(key)}" ${draft(key,fallback)?"checked":""}>${esc(label)}</label>${help?`<p class="field-help">${esc(help)}</p>`:""}`; }

  async function loadMemory(){try{app.memory=await api("memory");app.memoryError="";}catch(e){app.memoryError=e.message;}render();}
  function memoryPage(){
    const rows=(app.memory?.items||[]).filter(r=>state().filters.memory_archived||r.state!=="archived"),selected=rows.find(r=>r.id===state().filters.memory_id);
    return heading("What this individual remembers.","Lasting knowledge with its source, kept within the conversation where it belongs.",button("Add memory","memory.new",{primary:true,icon:"plus"}))+(app.memoryError?banner(app.memoryError,"error"):"")+
      `<section class="card section-gap"><div class="card-header"><h2>Knowledge</h2>${button(state().filters.memory_archived?"Hide archived":"Show archived","memory.archived",{small:true})}</div>${rows.length?`<ul class="work-list">${rows.map(r=>`<li><button class="work-select" data-action="memory.select" data-id="${esc(r.id)}"><strong>${esc(r.key)}</strong><span class="small muted">${r.audience.kind==="owner"?"Private owner context":esc(r.audience.provider_id+" · "+r.audience.conversation_id)} · ${esc(displayTime(r.updated_at))}</span></button>${badge(r.state==="conflicted"?"Conflicting accounts":title(r.state),r.state==="conflicted"?"amber":"")}</li>`).join("")}</ul>`:empty("Nothing remembered yet","Add useful lasting knowledge here, or let the agent record it from a connected conversation.","profile")}</section>`+
      (selected?`<section class="card section-gap" data-control="memory.detail"><div class="card-header"><h2>${esc(selected.key)}</h2>${button("Close","memory.select",{quiet:true,attrs:'data-id=""'})}</div>${selected.state==="conflicted"?banner("Different accounts are preserved below. No answer is selected until the evidence or a person resolves the conflict.","warning"):""}${selected.candidates.map(c=>`<article class="execution-run"><p class="work-goal">${esc(c.statement)}</p>${c.id===selected.selected_candidate_id?badge("Selected account","green"):""}<ul>${c.sources.map(s=>`<li>${esc(s.note)} <span class="small muted">· ${esc(s.recorded_by)} · ${esc(displayTime(s.recorded_at))}</span>${s.source?.url&&safeLink(s.source.url)?` <a href="${esc(safeLink(s.source.url))}" target="_blank" rel="noopener noreferrer">Source</a>`:""}</li>`).join("")}</ul>${selected.state!=="archived"&&c.id!==selected.selected_candidate_id?button("Select this account","memory.action",{small:true,attrs:`data-id="${esc(selected.id)}" data-command="resolve" data-candidate="${esc(c.id)}"`}):""}</article>`).join("")}<div class="work-actions">${selected.state!=="archived"?button("Add another account","memory.new",{small:true,attrs:`data-id="${esc(selected.id)}"`})+button("Archive","memory.action",{small:true,attrs:`data-id="${esc(selected.id)}" data-command="archive"` }):""}</div><p class="card-note">Sources and selections are recorded accounts. They do not grant authority or independently establish truth. Revision ${selected.revision}.</p></section>`:"");
  }
  function memoryDialog(){const edit=state().modal==="memory-command";return (edit?field("Why is this the right change?","memory.reason","",{textarea:true,rows:3}):field("Topic","memory.key","",{help:"Use the same short topic for later updates, such as project release date."})+field("What should be remembered?","memory.statement","",{textarea:true,rows:4})+field("Where did this come from?","memory.source_note","",{textarea:true,rows:2,help:"Name the conversation, document, observation, or correction."})+choice("Keep within","memory.audience","owner",[["owner","Private owner context"],["conversation","One conversation"]])+ (draft("memory.audience","owner")==="conversation"?field("Conversation provider","memory.provider","")+field("Conversation ID","memory.conversation",""):""))+`<div class="form-footer">${button("Cancel","modal.close",{quiet:true})}${button(edit?"Save change":"Remember","memory.save",{primary:true})}</div>`;}
  async function openMemory(payload){const old=app.memory?.items.find(r=>r.id===payload.id);await patchView(next=>{next.draft.memory={key:old?.key||"",statement:"",source_note:"",audience:old?.audience.kind||"owner",provider:old?.audience.provider_id||"",conversation:old?.audience.conversation_id||""};next.modal="memory-new";});}
  async function saveMemory(){await flushDrafts();const d=(k)=>String(draft("memory."+k,""));let saved;if(state().modal==="memory-command"){const f=state().filters; saved=await api(`memory/${encodeURIComponent(f.memory_id)}/commands`,{method:"POST",body:{expected_revision:f.memory_revision,command:{type:f.memory_action,...(f.memory_action==="resolve"?{candidate_id:f.memory_candidate}:{}),reason:d("reason")}}});}else{const audience=d("audience")==="conversation"?{kind:"conversation",provider_id:d("provider"),conversation_id:d("conversation")}:{kind:"owner"};saved=await api("memory",{method:"POST",body:{key:d("key"),statement:d("statement"),source_note:d("source_note"),audience,source:null}});}await command({type:"close_modal"});await command({type:"set_filter",key:"memory_id",value:saved.id});await loadMemory();setNotice(saved.state==="conflicted"?"Both accounts saved. Review the conflict before relying on either.":"Memory saved.");}

  const workStates={proposed:"Proposed",accepted:"Accepted",in_progress:"In progress",blocked:"Blocked",review:"Ready for review",completed:"Reported complete",cancelled:"Cancelled"};
  const workDrafts=["completion.criteria","completion.reviewers","completion.source","completion.artifact","completion.label","completion.uri","completion.version","completion.sha256","completion.note","completion.outcome","completion.checked","completion.evidence","memory.key","memory.statement","memory.source_note","memory.audience","memory.provider","memory.conversation","memory.reason","native.auth","native.account_file","account.location","account.callback_port","native.kind","native.model","native.endpoint","native.key_env","native.mode","native.command_network","native.images","native.audio","native.pdf","native.computer","native.tools","native.decisions","native.raw","scheduler.enabled","scheduler.paused","scheduler.concurrency","scheduler.approved","scheduler.session_mode","execution.seconds","execution.turns","execution.bytes","execution.tools","execution.note","execution.allow_retry","execution.request_id","work.title","work.goal","work.assignment","work.reason","work.summary","work.evidence_label","work.evidence_uri","work.evidence_kind","work.verify_note","work.verify_outcome","work.relationship","work.disclosure","work.import_packet","organization.id","organization.name","organization.version","organization.instructions","organization.domain","organization.purpose","organization.template","organization.audience","organization.provider","organization.conversation","organization.peer","organization.assignments","organization.send","organization.receive","organization.details","organization.active","organization.expected_revision"];
  function choice(label,key,value,options,help="") {return `<label class="field"><span class="field-label">${esc(label)}</span><select class="select" data-draft="${esc(key)}" data-action="edit-draft" data-control="${esc(key)}">${options.map(([id,name])=>`<option value="${esc(id)}" ${String(draft(key,value))===String(id)?"selected":""}>${esc(name)}</option>`).join("")}</select>${help?`<span class="field-help">${esc(help)}</span>`:""}</label>`;}
  function profileUseCard(){
    const p=app.profileUse;if(app.profileUseError)return banner(app.profileUseError,"warning");if(!p)return "";
    const notConfigured=p.status==="not_configured"||p.installed?.configured===false;
    const labels={not_configured:"Not configured",unknown:"No turn observed",partial:"Some context evidence missing",stale:"Last observed turn is older",supplied_current:"Current guidance supplied",supplied_previous:"Earlier guidance supplied"};
    const installed=notConfigured?"No persona profile configured":p.installed?.package?.id?shortId(p.installed.package.id):"Local guidance · no imported version";
    const note=notConfigured?"Add guidance here or import a profile from Persona Workshop. Any previous turn evidence is retained.":`${p.status==="stale"?"The last turn is over 24 hours old. A new turn will provide fresh evidence. ":""}${p.content_matches===true&&p.version_matches===false?"The text matches, but the installed version changed since that turn. ":""}This records context delivery. Whether the agent follows the guidance needs a separate behavioral check.`;
    return `<section class="card section-gap" data-control="profile.use"><div class="card-header"><div><h2>Profile in use</h2><p>Installed guidance and what reached the latest started turn.</p></div>${badge(labels[p.status]||"Unknown",notConfigured?"":p.status==="supplied_current"?"green":"amber")}</div><dl class="detail-list"><div class="detail-row"><dt>Installed profile</dt><dd>${esc(installed)}</dd></div><div class="detail-row"><dt>Latest started turn</dt><dd>${p.relevant_turn?esc(displayTime(p.relevant_turn.started_at)):"No evidence yet"}</dd></div>${p.relevant_turn?`<div class="detail-row"><dt>Session</dt><dd>${esc(shortId(p.relevant_turn.session_id))}</dd></div>`:""}</dl><p class="card-note">${note}</p></section>`;
  }
  function completionPanel(w){
    const c=w.completion_contract,active=!["completed","cancelled"].includes(w.state);
    const act=(label,type)=>button(label,"completion.open",{small:true,attrs:`data-id="${esc(w.id)}" data-command="${type}"`});
    if(!c)return `<section data-control="completion.summary"><h3>Acceptance</h3><p class="small muted">For work that needs named reviewers, capture the requirements and delivered versions.</p>${active?act("Set acceptance criteria","freeze_brief"):""}</section>`;
    const b=c.brief,artifacts=c.artifact_set?.artifacts||[];
    return `<section data-control="completion.summary"><h3>${c.closure?"Accepted by requester":"Acceptance and review"}</h3><ol>${(b.acceptance_criteria||[]).map(x=>`<li>${esc(x)}</li>`).join("")}</ol><p class="small">${artifacts.length} delivered version${artifacts.length===1?"":"s"} · ${b.reviewer_actors.length} named reviewer${b.reviewer_actors.length===1?"":"s"}</p>${artifacts.map(a=>`<p class="small">${esc(a.artifact)} · version ${esc(a.version)}</p>`).join("")}<p class="small muted">${c.closure?esc(c.closure.note||"Requester acceptance recorded."):"Each named reviewer checks the exact delivered versions. The requester then accepts the result."}</p>${active?`<div class="work-actions">${act("Revise criteria","freeze_brief")}${act("Record delivered version","artifact_version")}${w.state==="review"?act("Record artifact review","review_artifacts")+act("Accept and close","close"):""}</div>`:""}<details><summary>Review history and version details</summary><pre class="code-block">${esc(JSON.stringify(c,null,2))}</pre></details></section>`;
  }
  async function openCompletion(payload){
    await loadWork();const w=app.work.items.find(w=>w.id===payload.id);if(!w)throw new Error("Responsibility no longer exists.");
    if(!["freeze_brief","artifact_version","review_artifacts","close"].includes(payload.command))throw new Error("Unknown acceptance action.");
    const c=w.completion_contract;
    await patchView(next=>{next.filters.work_id=w.id;next.filters.completion_action=payload.command;next.filters.completion_revision=w.revision;next.filters.completion_brief=c?.brief.sha256||null;next.filters.completion_artifacts=c?.artifact_set.sha256||null;next.draft.completion={criteria:(c?.brief.acceptance_criteria||[]).join("\n"),reviewers:(c?.brief.reviewer_actors||[]).join("\n"),source:w.source?.url||"",artifact:"",label:"",uri:"",version:"",sha256:"",note:"",outcome:"pass",checked:false,evidence:(w.evidence||[]).filter(e=>["test","review"].includes(e.kind)).map(e=>e.id).join(", ")};next.modal="completion";});
  }
  function completionDialog(){
    const type=state().filters.completion_action,w=app.work?.items.find(w=>w.id===state().filters.work_id);let body="",label="Save";
    if(type==="freeze_brief"){
      body=field("What must be true when this is done?","completion.criteria","",{textarea:true,rows:5,help:"One acceptance criterion per line."})+field("Named reviewers","completion.reviewers","",{textarea:true,rows:2,help:"One authenticated reviewer principal per line, for example token: followed by its ID. Reviewers must be different from the author of delivered versions."})+field("Original request or brief link","completion.source","",{help:"Use a specific source URL. Existing source messages are retained when this is empty."});label="Save acceptance brief";
    }else if(type==="artifact_version"){
      body=field("Deliverable name","completion.artifact","")+field("Description","completion.label","")+field("Delivered version link","completion.uri","")+field("Version","completion.version","")+field("SHA-256 of the delivered file","completion.sha256","",{help:"Use the digest of the actual file bytes. Recording it does not independently verify the content."});label="Record this version";
    }else{
      body=`<ol>${(w?.completion_contract?.brief.acceptance_criteria||[]).map(c=>`<li>${esc(c)}</li>`).join("")}</ol>`;
      if(type==="review_artifacts")body+=choice("Review result","completion.outcome","pass",[["pass","Passed"],["fail","Needs changes"]])+field("Evidence IDs","completion.evidence","",{help:(w?.evidence||[]).filter(e=>["test","review"].includes(e.kind)).map(e=>`${e.id}: ${e.label}`).join(" · ")||"Add test or review evidence to this responsibility first."})+checkbox("I checked every criterion against these delivered versions","completion.checked",false);
      else body+=`<p>Your acceptance closes this responsibility only after its named reviewers pass. The service checks your requester identity and current versions.</p>`+checkbox("I accept the delivered result for this scope","completion.checked",false);
      body+=field(type==="close"?"Acceptance note":"What did you check?","completion.note","",{textarea:true,rows:4});label=type==="close"?"Accept and close":"Record review";
    }
    return body+`<div class="form-footer">${button("Cancel","modal.close",{quiet:true})}${button(label,"completion.save",{primary:true,disabled:app.workSaving})}</div>`;
  }
  async function saveCompletion(){
    if(app.workSaving)return;await flushDrafts();app.workSaving=true;render();
    try{
      const f=state().filters,w=app.work.items.find(w=>w.id===f.work_id),type=f.completion_action,d=k=>draft("completion."+k,""),cmd={type},lines=k=>String(d(k)).split(/\r?\n/).map(s=>s.trim()).filter(Boolean);
      if(type==="freeze_brief"){
        Object.assign(cmd,{acceptance_criteria:lines("criteria"),reviewer_actors:lines("reviewers")});
        if(String(d("source")).trim())cmd.source={provider_id:w.audience.kind==="conversation"?w.audience.provider_id:w.source?.provider_id||"owner",conversation_id:w.audience.kind==="conversation"?w.audience.conversation_id:w.source?.conversation_id||w.id,url:String(d("source")).trim()};
      }else if(type==="artifact_version"){
        for(const k of ["artifact","label","uri","version","sha256"])cmd[k]=String(d(k)).trim();
      }else{
        if(!d("checked"))throw new Error("Confirm your review or acceptance before saving.");
        Object.assign(cmd,{brief_sha256:f.completion_brief,artifact_set_sha256:f.completion_artifacts,note:d("note")});
        if(type==="review_artifacts")Object.assign(cmd,{outcome:d("outcome"),criterion_indices:w.completion_contract.brief.acceptance_criteria.map((_,i)=>i),evidence_ids:String(d("evidence")).split(",").map(s=>s.trim()).filter(Boolean)});
      }
      await api(`work/${encodeURIComponent(w.id)}/commands`,{method:"POST",body:{expected_revision:f.completion_revision,command:cmd}});
      await command({type:"close_modal"});await loadWork();setNotice(type==="close"?"Result accepted and responsibility closed.":"Acceptance record updated.");
    }finally{app.workSaving=false;render();}
  }
  function workPage(){const rows=app.work?.items||[],selected=rows.find(w=>w.id===state().filters?.work_id);return heading("Work that carries forward.","Responsibilities stay with this individual across conversations.",button("Add responsibility","work.new",{primary:true,icon:"plus"}))+ (app.workError?banner(app.workError,"error"):"")+`<div class="metrics">${metric("Open",rows.filter(w=>!["completed","cancelled"].includes(w.state)).length,"Recorded responsibilities","profile")}${metric("Blocked",rows.filter(w=>w.state==="blocked").length,"Needs attention","info")}${metric("Ready for review",rows.filter(w=>w.state==="review").length,"Check evidence before completion","check")}</div>${schedulerCard()}${selected?workDetail(selected):""}<section class="card section-gap"><div class="card-header"><div><h2>Responsibilities</h2><p>A record keeps track of work; creating one does not start an agent turn.</p></div>${button("Receive delegation","work.import.open",{small:true})}</div>${rows.length?`<ul class="work-list">${rows.map(w=>`<li><button class="work-select" data-action="work.select" data-id="${esc(w.id)}"><strong>${esc(w.title)}</strong><span class="small muted">${esc(w.assignment?.name||"Individual responsibility")} · ${esc(displayTime(w.updated_at))}</span></button>${badge(workStates[w.state]||w.state,w.state==="blocked"?"amber":w.state==="completed"?"green":"")}</li>`).join("")}</ul>`:empty("Nothing assigned yet","Add a responsibility here or ask this agent to record authorized work in a connected conversation.","profile")}</section>`;}
  function workDetail(w){const active=!["completed","cancelled"].includes(w.state),actions=[];if(w.state==="proposed")actions.push(["Accept","accept"]);if(["accepted","review"].includes(w.state))actions.push(["Start work","start"]);if(w.state==="blocked")actions.push(["Resume","resume"]);if(["accepted","in_progress"].includes(w.state))actions.push(["Ready for review","review"]);if(w.state==="review"&&!w.completion_contract)actions.push(["Record check","verify"]);if(w.state==="review"&&!w.completion_contract&&w.verification?.outcome==="pass")actions.push(["Mark complete","complete"]);if(active){actions.push(["Update progress","progress"],["Add evidence","evidence"],["Edit goal","amend"]);if(w.state!=="proposed")actions.push(["Mark blocked","block"]);if(w.assignment_id&&!w.upstream)actions.push(["Prepare delegation","export"]);actions.push(["Cancel","cancel"]);}else actions.push(["Reopen","reopen"]);return `<section class="card section-gap" data-control="work.detail"><div class="card-header"><div><h2>${esc(w.title)}</h2><p>${esc(w.assignment?.name||"Individual responsibility")} · ${esc(workStates[w.state])}</p></div>${button("Close","work.select",{quiet:true,attrs:'data-id=""'})}</div><p class="work-goal">${esc(w.goal)}</p>${w.blocker?banner(w.blocker,"warning"):""}${w.progress?`<p>${esc(w.progress)}</p>`:""}${w.upstream?banner("Received as a proposal. Source identity and remote claims are unverified; review the work locally before accepting.","warning"):""}<div class="work-actions">${actions.map(([label,action])=>button(label,"work.action",{small:true,attrs:`data-id="${esc(w.id)}" data-command="${action}" data-revision="${w.revision}"`})).join("")}</div>${executionPanel(w)}${completionPanel(w)}<h3>Evidence</h3>${w.evidence.length?`<ul>${w.evidence.map(e=>`<li>${safeLink(e.uri)?`<a href="${esc(safeLink(e.uri))}" target="_blank" rel="noopener noreferrer">${esc(e.label)}</a>`:esc(e.label)} <span class="small muted">${esc(e.kind)} · recorded reference</span></li>`).join("")}</ul>`:`<p class="small muted">No evidence recorded.</p>`}${w.verification?`<p class="small">Recorded check: <strong>${esc(w.verification.outcome)}</strong> · ${esc(w.verification.note)}</p>`:""}<p class="card-note">Owner: ${esc(shortId(w.owner_agent_id))} · Revision ${w.revision}. Completion records an authenticated report; execution reports and recorded references are not independently validated evidence.</p></section>`;}
  const executionLabels={queued:"Queued",starting:"Starting",running:"Working",cancelling:"Stopping",reported:"Report ready",blocked:"Needs input",paused_limit:"Turn limit reached",needs_reconciliation:"Check before retry",failed_before_prompt:"Did not start",cancelled:"Cancelled",reconciled:"Interruption reviewed"};
  function schedulerCard(){const s=app.scheduler;if(!s)return app.schedulerError?banner(app.schedulerError,"warning"):"";const c=s.settings;return `<section class="card section-gap" data-control="scheduler.summary"><div class="card-header"><div><h2>Background work</h2><p>Authorize a responsibility to keep working after you close this page.</p></div>${button("Settings","scheduler.open",{small:true})}</div><p>${badge(!c.enabled?"Off":c.paused?"Paused":"Enabled",c.enabled&&!c.paused?"green":"")} · ${s.counts.queued||0} queued · ${(s.counts.running||0)+(s.counts.starting||0)} active · ${s.counts.needs_reconciliation||0} need inspection</p><p class="card-note">Each run has its own budget. Reports wait for review; nothing is sent to other conversations.</p></section>`;}
  function executionPanel(w){const runs=app.execution?.work_id===w.id?app.execution.runs:[],canRun=["accepted","in_progress"].includes(w.state);return `<section class="execution-panel" data-control="execution.runs"><div class="card-header"><h3>Execution</h3>${canRun?button("Authorize a run","execution.open",{small:true,attrs:`data-command="queue" data-id="${esc(w.id)}"`}):""}</div>${runs.length?runs.map(r=>`<article class="execution-run"><div class="card-header"><strong>${esc(executionLabels[r.state]||r.state)}</strong><span class="small muted">${esc(displayTime(r.updated_at))}</span></div><p class="small">${r.turns_completed||0}/${r.budget.max_turns||1} turns · ${r.output_bytes||0} bytes returned · ${r.tool_calls||0} observed tool calls · ${r.budget.max_seconds}s budget · ${esc(r.session_mode||"Engine default")} mode${r.mode_applied?" (applied)":""}</p>${r.progress?`<p class="small">${esc(r.progress)}</p>`:""}${r.reason?`<p class="small muted">${esc(title(r.reason))}</p>`:""}${r.state==="needs_reconciliation"?banner("This attempt may have changed files. Inspect its report and workspace before deciding whether another attempt is safe.","warning"):""}${r.report?`${button(state().filters.execution_report===r.id?"Hide report":"Read the agent’s report","execution.report",{small:true,attrs:`data-id="${esc(r.id)}"`})}${state().filters.execution_report===r.id?`<pre class="execution-report">${esc(r.report)}</pre><p class="small muted">Agent report · has not been independently checked.</p>`:""}`:""}${r.reconciliation?`<p class="small">Operator review: ${esc(r.reconciliation.note)}</p>`:""}<div class="work-actions">${["queued","starting","running","cancelling"].includes(r.state)?button("Cancel run","execution.open",{small:true,attrs:`data-command="cancel" data-id="${esc(w.id)}" data-run="${esc(r.id)}"`}):""}${r.state==="needs_reconciliation"?button("Record inspection","execution.open",{small:true,attrs:`data-command="reconcile" data-id="${esc(w.id)}" data-run="${esc(r.id)}"`}):""}${canRun&&r.load_session_supported&&["reported","blocked","paused_limit"].includes(r.state)?button("Continue saved session","execution.open",{small:true,attrs:`data-command="resume" data-id="${esc(w.id)}" data-run="${esc(r.id)}"`}):""}${canRun&&(["failed_before_prompt","cancelled","reported","blocked","paused_limit"].includes(r.state)||(r.state==="reconciled"&&r.reconciliation?.allow_retry))?button("Authorize another attempt","execution.open",{small:true,attrs:`data-command="retry" data-id="${esc(w.id)}" data-run="${esc(r.id)}"`}):""}</div></article>`).join(""):`<p class="small muted">No background runs. Accept the responsibility, then authorize a bounded attempt.</p>`}</section>`;}
  function schedulerDialog(){return `<p>Choose whether explicitly authorized tasks can run without an open conversation. Pausing keeps current attempts running; switching off stops them and may require inspection.</p>`+checkbox("Enable background work","scheduler.enabled",false)+checkbox("Pause new starts","scheduler.paused",false)+field("Concurrent runs","scheduler.concurrency",1,{type:"number",min:1,max:4})+checkbox("This engine is restricted to the local workspace work I authorize","scheduler.approved",false)+`<p class="field-help">The agent’s engine controls its built-in tools. Background runs cannot approve extra permissions or use conversation connectors. Changing the engine requires renewed approval.</p>${button(state().filters.scheduler_advanced?"Hide advanced settings":"Advanced settings","scheduler.advanced",{small:true})}${state().filters.scheduler_advanced?field("ACP session mode · optional","scheduler.session_mode","",{placeholder:"Engine default",help:"For example workspace or read_only, if offered by this engine. This authorizes that mode for newly queued background runs only. Existing runs keep their chosen mode; interactive conversations stay unchanged."}):""}<div class="form-footer">${button("Cancel","modal.close",{quiet:true})}${button("Save settings","scheduler.save",{primary:true})}</div>`;}
  function executionDialog(){const action=state().filters?.execution_action;let body="";if(["queue","retry","resume"].includes(action)){body=`<p>Authorize a bounded series of working turns. The agent keeps its session, reports progress, and stops for a blocker, review, or its limits. Resuming loads the saved session; a fresh attempt starts a new one.</p>`+field("Time limit · seconds","execution.seconds",300,{type:"number",min:1,max:3600})+field("Maximum working turns","execution.turns",1,{type:"number",min:1,max:12,help:"Time, output and tool limits apply to the entire run."})+`${button(state().filters.execution_limits?"Hide other limits":"Other limits","execution.limits",{small:true})}${state().filters.execution_limits?`${field("Report size · bytes","execution.bytes",131072,{type:"number",min:1024,max:1048576})}${field("Observed tool calls","execution.tools",30,{type:"number",min:0,max:100})}`:""}`;}if(action!=="queue")body+=field(action==="reconcile"?"What did you inspect and find?":"Reason","execution.note","",{textarea:true,rows:4});if(action==="reconcile")body+=checkbox("Allow a separately authorized retry after this inspection","execution.allow_retry",false);return body+`<div class="form-footer">${button("Cancel","modal.close",{quiet:true})}${button(["queue","retry","resume"].includes(action)?"Authorize run":action==="cancel"?"Stop attempt":"Record inspection","execution.save",{primary:true})}</div>`;}
  async function saveScheduler(){await flushDrafts();const enabled=!!draft("scheduler.enabled",false),approved=!!draft("scheduler.approved",false);if(enabled&&!approved)throw new Error("Confirm the engine is restricted to authorized local work before enabling it.");await api("scheduler",{method:"PUT",body:{expected_revision:state().filters.scheduler_revision,enabled,paused:!!draft("scheduler.paused",false),concurrency:Number(draft("scheduler.concurrency",1)),approved_backend_sha256:approved?state().filters.scheduler_backend:null,session_mode:String(draft("scheduler.session_mode","")).trim()||null}});await command({type:"close_modal"});await loadWork();setNotice("Background work settings saved.");}
  async function openExecution(payload){await command({type:"set_filter",key:"work_id",value:payload.id});await loadWork();const w=app.work.items.find(w=>w.id===payload.id),run=app.execution?.runs.find(r=>r.id===(payload.run||payload.run_id));if(!w)throw new Error("Responsibility no longer exists.");await patchView(next=>{next.filters.execution_action=payload.command;next.filters.execution_run=run?.id||null;next.filters.execution_revision=run?.revision||null;next.filters.execution_work_revision=w.revision;next.draft.execution={seconds:run?.budget.max_seconds||300,turns:run?.budget.max_turns||1,bytes:run?.budget.max_output_bytes||131072,tools:run?.budget.max_tool_calls??30,note:"",allow_retry:false,request_id:crypto.randomUUID()};next.modal="execution";});}
  async function saveExecution(){await flushDrafts();const f=state().filters,type=f.execution_action,body={type},id=f.work_id;if(["queue","retry","resume"].includes(type))Object.assign(body,{expected_work_revision:f.execution_work_revision,request_id:String(draft("execution.request_id")),budget:{max_seconds:Number(draft("execution.seconds",300)),max_turns:Number(draft("execution.turns",1)),max_output_bytes:Number(draft("execution.bytes",131072)),max_tool_calls:Number(draft("execution.tools",30))}});if(type!=="queue")Object.assign(body,{run_id:f.execution_run,expected_revision:f.execution_revision});if(type==="reconcile")Object.assign(body,{note:String(draft("execution.note")),allow_retry:!!draft("execution.allow_retry",false)});if(["cancel","retry","resume"].includes(type))body.reason=String(draft("execution.note"));await api(`work/${encodeURIComponent(id)}/execution`,{method:"POST",body});await command({type:"close_modal"});await loadWork();setNotice(["queue","retry","resume"].includes(type)?"Run authorized. You can close this page; execution belongs to the service.":"Execution record updated.");}
  function organizationPage(){const org=app.organization?.organization||{templates:[],assignments:[],relationships:[]};return heading("One individual, several roles.","Reuse a discipline, assign it a purpose, and decide explicitly what can cross a relationship.")+ (app.workError?banner(app.workError,"error"):"")+[["templates","Disciplines","Reusable instructions, versioned separately from this individual.","template"],["assignments","Assignments","A role, purpose and conversation boundary for this agent.","assignment"],["relationships","Sharing relationships","Named peers and selected assignments. Coordination grants no implied or onward access.","relationship"]].map(([key,label,copy,kind])=>`<section class="card section-gap"><div class="card-header"><div><h2>${label}</h2><p>${copy}</p></div>${button("Add "+kind,"organization.edit",{small:true,attrs:`data-kind="${kind}"`})}</div>${org[key].length?`<ul class="work-list">${org[key].map(r=>`<li><div><strong>${esc(r.name||r.label)}</strong><p class="small muted">${esc(kind==="template"?`Version ${r.version}`:kind==="assignment"?`${r.domain} · ${r.audience.kind==="owner"?"Private to the owner":"Selected conversation"}`:`${r.send?"Can send":"No sending"} · ${r.receive?"Can receive":"No receiving"} · ${r.allow_details?"Details permitted":"Summary only"}`)}${r.active===false?" · Archived":""}</p></div>${button("Edit","organization.edit",{small:true,attrs:`data-kind="${kind}" data-id="${esc(r.id)}"`})}</li>`).join("")}</ul>`:empty("None configured",copy,"profile")}</section>`).join("");}
  function workDialog(name){const w=app.work?.items.find(w=>w.id===state().filters?.work_id),action=state().filters?.work_action,org=app.organization?.organization;let body="",label="Save";if(name==="work-new"){label="Add responsibility";body=field("Title","work.title","",{required:true})+field("What should be accomplished?","work.goal","",{textarea:true,rows:4})+choice("Assignment","work.assignment","",[["","Individual · private"],...(org?.assignments||[]).filter(a=>a.active).map(a=>[a.id,a.name])]);}else if(name==="work-import"){label="Receive proposal";body=choice("Relationship","work.relationship","",[["","Choose a relationship"],...(org?.relationships||[]).filter(r=>r.active&&r.receive).map(r=>[r.id,r.label])])+choice("Private assignment","work.assignment","",[["","Choose an assignment"],...(org?.assignments||[]).filter(a=>a.active&&a.audience.kind==="owner").map(a=>[a.id,a.name])])+field("Delegation envelope","work.import_packet","",{textarea:true,rows:8,help:"Paste the exported JSON from an explicitly authorized source. Receiving records a proposal and does not confirm the sender's identity."});}else if(action==="evidence"){body=field("What does this evidence show?","work.evidence_label","")+field("Reference","work.evidence_uri","",{placeholder:"https://… or file:///…"})+choice("Kind","work.evidence_kind","artifact",[["artifact","Artifact"],["test","Test result"],["review","Review"],["source","Source"]]);}else if(action==="verify"){body=choice("Result","work.verify_outcome","pass",[["pass","Passed"],["fail","Failed"]])+field("What did you check?","work.verify_note","",{textarea:true,rows:4,help:"This records your check against the listed evidence. It does not claim an independent check occurred."});}else if(action==="export"){label="Prepare envelope";body=choice("Recipient relationship","work.relationship","",[["","Choose a relationship"],...(org?.relationships||[]).filter(r=>r.active&&r.send&&r.assignment_ids.includes(w?.assignment_id)).map(r=>[r.id,r.label])])+choice("Share","work.disclosure","summary",[["summary","Only this summary"],["details","Summary, goal and evidence"]])+field("Summary for this peer","work.summary","",{textarea:true,rows:4,help:"Include only what this recipient should see. Preparing an envelope does not send it."});}else if(action==="amend"){body=field("Title","work.title",w?.title||"")+field("Goal","work.goal",w?.goal||"",{textarea:true,rows:4});}else if(action==="progress"){body=field("Progress","work.summary",w?.progress||"",{textarea:true,rows:4});}else{body=field("Reason","work.reason","",{textarea:true,rows:3});}return body+`<div class="form-footer">${button("Cancel","modal.close",{quiet:true})}${button(label,"work.save",{primary:true,disabled:app.workSaving})}</div>`;}
  function organizationDialog(kind){const org=app.organization?.organization;let body=field("Name","organization.name","");if(kind==="template")body+=field("Version","organization.version","1")+field("Discipline instructions","organization.instructions","",{textarea:true,rows:6});if(kind==="assignment")body+=field("Domain","organization.domain","work",{help:"For example work, personal or a project name."})+field("Purpose","organization.purpose","",{textarea:true,rows:3})+choice("Discipline","organization.template","",[["","No discipline"],...(org?.templates||[]).map(t=>[t.id,t.name])])+choice("Audience","organization.audience","owner",[["owner","Private to the owner"],["conversation","One connected conversation"]])+field("Conversation provider ID","organization.provider","",{help:"Only needed for a conversation audience."})+field("Conversation ID","organization.conversation","",{help:"Use the source's identifier, not a display name."});if(kind==="relationship")body+=field("Peer agent ID","organization.peer","")+field("Assignments to include","organization.assignments","",{help:"Comma-separated assignment IDs. Available: "+(org?.assignments||[]).map(a=>`${a.id} (${a.name})`).join(", ")})+checkbox("Allow preparing delegations","organization.send",false)+checkbox("Allow receiving delegations","organization.receive",false)+checkbox("Allow goal and evidence details","organization.details",false,"Otherwise only an explicitly written summary can be shared.");if(kind!=="template")body+=checkbox("Active","organization.active",true);return body+`<div class="form-footer">${button("Cancel","modal.close",{quiet:true})}${button("Save "+kind,"organization.save",{primary:true,attrs:`data-kind="${kind}"`})}</div>`;}
  async function loadWork(){try{[app.work,app.organization]=await Promise.all([api("work"),api("organization")]);app.workError="";}catch(e){app.workError=e.message;}try{app.scheduler=await api("scheduler");app.schedulerError="";const id=state().filters?.work_id;app.execution=id?await api(`work/${encodeURIComponent(id)}/execution`):null;}catch(e){app.schedulerError=e.message;}render();}
  async function editOrganization(payload){await loadWork();const kind=payload.kind,key={template:"templates",assignment:"assignments",relationship:"relationships"}[kind];if(!key)throw new Error("Unknown organization type");const r=app.organization.organization[key].find(r=>r.id===payload.id)||{};await patchView(next=>{next.draft.organization={id:r.id||"",name:r.name||r.label||"",version:r.version||"1",instructions:r.instructions||"",domain:r.domain||"work",purpose:r.purpose||"",template:r.template_id||"",audience:r.audience?.kind||"owner",provider:r.audience?.provider_id||"",conversation:r.audience?.conversation_id||"",peer:r.peer_agent_id||"",assignments:r.assignment_ids?.join(", ")||"",send:r.send===true,receive:r.receive===true,details:r.allow_details===true,active:r.active!==false,expected_revision:app.organization.revision};next.modal="organization-"+kind;});}
  async function saveOrganization(kind){await flushDrafts();const org=clone(app.organization.organization),name=String(draft("organization.name","")),id=String(draft("organization.id",""))||name.toLowerCase().replace(/[^a-z0-9]+/g,"-").replace(/^-|-$/g,"").slice(0,65)+"-"+Math.random().toString(36).slice(2,8);let item,key;if(kind==="template"){key="templates";item={id,name,version:String(draft("organization.version","1")),instructions:String(draft("organization.instructions",""))};}else if(kind==="assignment"){key="assignments";item={id,name,domain:String(draft("organization.domain","work")),purpose:String(draft("organization.purpose","")),template_id:draft("organization.template","")||null,audience:draft("organization.audience","owner")==="owner"?{kind:"owner"}:{kind:"conversation",provider_id:String(draft("organization.provider","")),conversation_id:String(draft("organization.conversation",""))},active:draft("organization.active",true)};}else{key="relationships";item={id,label:name,peer_agent_id:String(draft("organization.peer","")),assignment_ids:String(draft("organization.assignments","")).split(",").map(s=>s.trim()).filter(Boolean),send:draft("organization.send",false),receive:draft("organization.receive",false),allow_details:draft("organization.details",false),active:draft("organization.active",true)};}const index=org[key].findIndex(r=>r.id===id);if(index<0)org[key].push(item);else org[key][index]=item;await api("organization",{method:"PUT",body:{expected_revision:Number(draft("organization.expected_revision",app.organization.revision)),organization:org}});await command({type:"close_modal"});await loadWork();setNotice("Saved "+kind+". Existing work keeps its captured instructions and audience.");}
  async function workAction(payload){const action=payload.command;await command({type:"set_filter",key:"work_id",value:payload.id});if(["accept","start","resume","review","complete"].includes(action)){await api(`work/${encodeURIComponent(payload.id)}/commands`,{method:"POST",body:{expected_revision:Number(payload.revision),command:{type:action}}});await loadWork();render();return;}await clearDraftNamespace("work");await patchView(next=>{next.filters.work_action=action;next.filters.work_revision=Number(payload.revision);next.modal="work-command";});}
  async function saveWork(){if(app.workSaving)return;app.workSaving=true;render();try{await flushDrafts();const name=state().modal,d=(k,f="")=>draft("work."+k,f),org=app.organization.organization;let result;
    if(name==="work-new"){const assignment=org.assignments.find(a=>a.id===d("assignment"));result=await api("work",{method:"POST",body:{title:d("title"),goal:d("goal"),assignment_id:assignment?.id||null,audience:assignment?.audience||{kind:"owner"}}});}
    else if(name==="work-import"){result=(await api("work/import",{method:"POST",body:{relationship_id:d("relationship"),assignment_id:d("assignment"),packet:JSON.parse(d("import_packet"))}})).work;}
    else{const id=state().filters.work_id,expected_revision=Number(state().filters.work_revision),type=state().filters.work_action,command={type};if(type==="export"){const packet=await api(`work/${encodeURIComponent(id)}/export`,{method:"POST",body:{expected_revision,relationship_id:d("relationship"),disclosure:d("disclosure","summary"),summary:d("summary")}});const blob=new Blob([JSON.stringify(packet,null,2)],{type:"application/json"}),url=URL.createObjectURL(blob),link=document.createElement("a");link.href=url;link.download=`${packet.id}.delegation.json`;link.click();setTimeout(()=>URL.revokeObjectURL(url),1000);}else{if(type==="evidence")Object.assign(command,{label:d("evidence_label"),uri:d("evidence_uri"),kind:d("evidence_kind","artifact")});if(type==="verify")Object.assign(command,{outcome:d("verify_outcome","pass"),note:d("verify_note"),evidence_ids:(app.work.items.find(w=>w.id===id)?.evidence||[]).map(e=>e.id)});if(type==="progress")command.summary=d("summary");if(type==="amend")Object.assign(command,{title:d("title",app.work.items.find(w=>w.id===id)?.title),goal:d("goal",app.work.items.find(w=>w.id===id)?.goal)});if(["block","cancel","reopen"].includes(type))command.reason=d("reason");result=await api(`work/${encodeURIComponent(id)}/commands`,{method:"POST",body:{expected_revision,command}});}}
    await command({type:"close_modal"});await clearDraftNamespace("work");await loadWork();if(result?.id)await command({type:"set_filter",key:"work_id",value:result.id});setNotice("Work record updated.");}finally{app.workSaving=false;render();}}

  function loginScreen() {
    return `<main class="login-screen"><section class="login-story"><div class="brand"><span class="brand-mark" aria-hidden="true">a<span>·</span></span><div>Agent Instance<small>One agent. Its own space.</small></div></div><div class="story"><div class="eyebrow story-eyebrow">Independent by design</div><h1>A place for your<br><span>agent to become.</span></h1><p>Give it a purpose. Choose how it works. Connect it to the experiences you use, and see what happens along the way.</p><div class="story-line"></div><p>One instance, with its own identity, configuration, and working context.</p></div><div class="caption">A small foundation for bigger ideas</div></section><section class="login-area"><form class="login-form" id="login-form"><div class="eyebrow">Welcome to your instance</div><h1>Make yourself at home.</h1><p class="login-copy">Sign in with the username and password for the machine account that owns this agent.</p>${app.error?banner(app.error,"error"):""}<label class="field"><span class="field-label">Machine username</span><input class="input" name="username" data-control="auth.username" autocomplete="username" required autofocus></label><label class="field"><span class="field-label">Password</span><input class="input" name="password" data-sensitive type="password" data-control="auth.password" autocomplete="current-password" required></label><button class="button primary" data-action="auth.login" type="submit" ${app.busy?"disabled":""}>${app.busy?"Signing in…":"Sign in to this agent"}${icon("arrow")}</button><p class="login-note">Your password is used for sign-in only. It is never saved in the agent’s configuration, profiles, or view history.</p><a class="help-link" href="welcome" data-control="certificate.help">Browser certificate help</a></form></section></main>`;
  }

  function shell(content) {
    const route=currentRoute(); const name=app.doc?.config.name || "New agent";
    return `<div class="shell"><aside class="sidebar"><div class="brand"><span class="brand-mark" aria-hidden="true">a<span>·</span></span><div>Agent Instance<small>Independent by design</small></div></div><div class="agent-switch"><div class="avatar">${portraitImage()}</div><div><div class="name">${esc(name)}</div><div class="small">Single agent workspace</div></div></div><div class="nav-caption">Workspace</div><nav aria-label="Agent navigation">${(!app.doc.config.setup_complete?["setup"]:[]).concat(routes).map(r=>`<button type="button" class="nav-link ${route===r?"active":""}" data-action="navigate" data-route="${r}" data-control="nav.${r}" ${route===r?'aria-current="page"':""}>${icon(listIcon(r))}${esc(r==="setup"?"Finish setup":titles[r])}${route===r?'<span class="nav-dot"></span>':""}</button>`).join("")}</nav><div class="sidebar-footer"><div class="live-status"><span class="status-dot ${app.online?"":"off"}"></span>${app.online?"Service connected":"Reconnecting to service"}</div><div class="small">${esc(app.status?.version ? `Version ${app.status.version}` : "Agent Instance")}<br>${esc(shortId(app.doc.config.instance_id))}</div><button class="logout" type="button" data-action="auth.logout">Sign out · ${esc(app.session?.owner || app.doc.config.owner)}</button></div></aside><main class="main"><header class="topbar"><div class="breadcrumb"><span>${esc(name)}</span>${icon("chevron")}<strong>${esc(titles[route])}</strong></div><div class="topbar-right"><span class="command-status" title="This browser has its own controllable view">${icon("screen")}<span data-view-revision>View ${esc(app.view?.revision||0)}</span></span><span class="identity">${esc(app.session?.owner||"")}</span>${badge(app.status?.enabled?"Agent enabled":"Agent stopped",app.status?.enabled?"green":"")}</div></header><div class="content">${app.error?banner(app.error,"error"):""}${app.notice?`<div class="banner">${icon("check")}<div>${esc(app.notice)}</div><button class="dismiss" type="button" data-action="notice.dismiss" aria-label="Dismiss notification">×</button></div>`:""}${content}<footer class="footer-meta"><span>One independent instance · ACP at the boundary</span><span>View ${esc(shortId(app.view?.id))} · Configuration revision ${esc(app.doc.revision)}</span></footer></div></main></div>${modal()}`;
  }

  function addressRows() {
    const a=app.status?.addresses||{}; const urls=Array.isArray(a.urls)?a.urls:[];
    return `<dl class="detail-list"><div class="detail-row"><dt>Open this agent</dt><dd>${urls.length?urls.map(u=>safeLink(u)?`<a href="${esc(safeLink(u))}" target="_blank" rel="noopener noreferrer">${esc(u)}</a>`:esc(u)).join("<br>"):esc(base.href)}</dd></div><div class="detail-row"><dt>Active listener</dt><dd class="mono">${esc(a.effective_server ? `${a.effective_server.bind}:${a.effective_server.port}` : "See active service URL")}</dd></div><div class="detail-row"><dt>Desired listener</dt><dd class="mono">${esc(app.doc.config.server.bind)}:${esc(app.doc.config.server.port||"automatic")}</dd></div><div class="detail-row"><dt>Server certificate SHA-256</dt><dd class="mono">${a.certificate_sha256?esc(a.certificate_sha256):"Fingerprint not supplied"}</dd></div></dl>`;
  }
  function attachInfo() { const a=app.status?.addresses||{}; const v=a.acp_attach ?? app.status?.acp_attach; return typeof v==="string"?v:v?JSON.stringify(v,null,2):"Connection instructions will appear when the service advertises its ACP entrypoint."; }
  function lifecycleButton() { return app.status?.enabled ? button("Stop agent","runtime.stop",{icon:"stop"}) : button("Start agent","runtime.start",{primary:true,icon:"play",disabled:!app.doc?.config.setup_complete}); }
  function metric(label,value,foot,ico) {return `<div class="metric"><div class="metric-label">${esc(label)}${icon(ico)}</div><div class="metric-value">${esc(value)}</div><div class="metric-foot">${esc(foot)}</div></div>`;}
  function timeline(events,limit=6) {
    const visible=events.filter(e=>e.category!=="diagnostics").slice(0,limit);
    if(!visible.length)return empty("A quiet start","Activity will appear as clients connect and the agent works.","activity");
    return `<div class="timeline">${visible.map(e=>`<div class="timeline-item"><div class="timeline-icon">${icon(String(e.kind).includes("error")?"info":"activity")}</div><button class="event-button" type="button" data-action="event.inspect" data-event="${esc(e.seq)}"><div class="timeline-title">${esc(eventSummary(e)||title(e.kind))}</div><div class="timeline-time">${esc(displayTime(eventTime(e)))} · ${esc(title(e.category||"activity"))}</div></button></div>`).join("")}</div>`;
  }
  function dashboard() {
    const s=app.status||{}, c=app.doc.config; const enabled=s.enabled;
    return heading("A little clarity, at a glance.","The identity, connections, and recent work of this one agent.",button("Refresh","refresh",{icon:"refresh"}))+(!c.setup_complete?`<div class="notice-strip"><p>This instance is installed. Complete its setup to begin accepting work.</p>${button("Continue setup","navigate",{primary:true,attrs:'data-route="setup"',icon:"arrow"})}</div>`:"")+`<section class="hero card"><div class="hero-identity"><div class="avatar">${portraitImage()}</div><div><h2>${esc(c.name)}</h2><p>${esc(c.purpose||"A new independent agent, ready for a purpose of its own.")}</p><div class="connection-line">${badge(enabled?"Ready for connections":"Runtime stopped",enabled?"green":"")}<span class="mono">${esc(c.instance_id)}</span></div></div></div><div class="hero-controls">${lifecycleButton()}</div></section><div class="metrics">${metric("ACP connections",app.connections.length,"Clients attached to this agent","connections")}${metric("Sessions",app.sessions.length,"Working contexts reported","profile")}${metric("Service uptime",age(s.uptime_ms),"Management service remains available","clock")}${metric("Configuration",`r${app.doc.revision}`,s.config_revision!==undefined&&s.config_revision!==app.doc.revision?`Active revision ${s.config_revision}`:"Saved configuration revision","settings")}</div>${profileUseCard()}<div class="grid asymmetric section-gap"><section class="card"><div class="card-header"><div><h2>Connect to this agent</h2><p>Use its ACP entrypoint from a client or adapter.</p></div>${badge("ACP","blue")}</div><div class="connection-code" data-control="connection.instructions">${esc(attachInfo())}</div>${button("Copy connection information","connection.copy",{small:true,icon:"copy"})}<p class="card-note">Adapters connect through ACP. Their Slack, Teams, or other platform settings belong to the adapter.</p></section><section class="card"><div class="card-header"><div><h2>Recent activity</h2><p>What this instance has been doing.</p></div>${button("View all","navigate",{quiet:true,small:true,attrs:'data-route="activity"'})}</div>${app.recentActivityError?banner(app.recentActivityError,"warning"):timeline(app.activityRecent,5)}</section></div><section class="card section-gap"><div class="card-header"><div><h2>At home on this machine</h2><p>Active addresses and the saved listener configuration.</p></div>${button("Edit settings","navigate",{quiet:true,small:true,attrs:'data-route="configuration"'})}</div><div class="grid two"><div>${addressRows()}</div><dl class="detail-list"><div class="detail-row"><dt>Runtime backend</dt><dd class="mono">${esc(app.backend?.name||"Not configured")}</dd></div><div class="detail-row"><dt>Installation owner</dt><dd>${esc(c.owner)}</dd></div><div class="detail-row"><dt>View connection</dt><dd>${app.streamOnline?"Live updates connected":"Live updates reconnecting"}</dd></div></dl></div></section>`;
  }

  function portraitImage(cls="") {return app.identity?`<img class="agent-portrait ${cls}" src="${new URL("identity/avatar",API)}?v=${esc(app.identity.avatar_revision)}" alt="${esc(app.doc?.config.name||"Agent")} portrait">`:esc(initial());}
  function portraitControls() {return `<div class="portrait-settings" data-control="identity.avatar"><div class="portrait-preview">${portraitImage()}</div><div><strong>Agent image</strong><p class="small muted">The face this agent uses across connected experiences. Square images work best; other shapes are cropped to the center.</p><label class="button"><span>${app.identity?.avatar.kind==="uploaded"?"Change image":"Upload image"}</span><input type="file" class="portrait-file" accept="image/png,image/jpeg,image/webp" data-control="identity.avatar.file" aria-label="Upload agent image"></label> ${app.identity?.avatar.kind==="uploaded"?button("Remove image","identity.avatar.remove",{quiet:true}):""}<p class="small muted">PNG, JPEG or WebP, up to 5 MB. Changes are saved immediately.</p></div></div>`;}
  async function savePortrait(payload) {if(!app.identity)app.identity=await api("identity");app.identity=await api("identity/avatar",{method:"PUT",body:{expected_revision:payload.expected_revision??app.identity.avatar_revision,media_type:payload.media_type,data:payload.data}});setNotice("Agent image saved. Connected experiences can pick up the change automatically.");}
  function identityFields() { return portraitControls()+configField("Agent name","name",{required:true,placeholder:"A name you’ll recognize"})+configField("Purpose","purpose",{textarea:true,rows:3,help:"What this individual is here to help with. A responsibility, a role, or an ongoing aim."})+configField("How it should work","instructions",{textarea:true,long:true,help:"Guidance for how the agent should work. Communication, working-style, and identity profiles can be edited separately."}); }
  function backendFields() {
    return configField("ACP backend executable","backend.command",{required:true,code:true,placeholder:"/path/to/acp-agent",help:"The executable must speak ACP over standard input/output. Arguments are passed directly, without a shell."})+`<div class="field-grid">${configField("Arguments · JSON array","backend.args",{json:true,array:true,rows:4,help:'One JSON string per argument, for example ["--mode", "work"].'})}${configField("Working directory","backend.working_directory",{code:true,required:true,default:"${AGENT_HOME}/workspace",help:"The backend’s starting directory. ${AGENT_HOME} refers to this installation."})}${configField("Allowed working roots · JSON array","backend.allowed_working_roots",{json:true,array:true,help:"Folders that clients may choose for their sessions. Use absolute paths or ${AGENT_HOME}."})}${configField("Non-secret environment · JSON object","backend.env",{json:true,help:"Only ordinary settings here. Use secret references for passwords, API keys, or tokens."})}${configField("Secret environment references · JSON object","backend.secret_env",{json:true,full:true,help:'Map an environment variable to a stored secret name, for example {"API_KEY": "provider-key"}. Add secret values in Access & credentials.'})}</div>`;
  }
  function serverFields() { return `<div class="field-grid">${configField("Bind address","server.bind",{code:true,help:"0.0.0.0 listens on available IPv4 interfaces. Use a specific address to restrict the listener."})}${configField("Port","server.port",{type:"number",min:0,max:65535,help:"0 chooses an available port. The service records the selected port for future starts."})}${configField("Certificate help port","server.welcome_port",{type:"number",min:0,max:65535,help:"0 chooses an available HTTP port for the certificate guide. Leave empty to disable. Sign-in and agent access remain HTTPS."})}${configField("Public URL","server.public_url",{full:true,placeholder:"https://machine.example:port/",help:"Optional HTTPS address for displayed connection information. A proxy prefix may be included."})}${app.status?.authentication==="windows_logon"?"":configField("PAM service","server.pam_service",{code:true,help:"The PAM profile used to authenticate the installation owner."})}${configField("Allowed embedding origins · JSON array","server.embed_origins",{json:true,array:true,help:'Exact HTTPS origins only, for example ["https://fleet.example"]. An empty list permits no external controller.'})}${configField("TLS certificate file","server.certificate_file",{code:true,help:"Optional certificate path. Supply both certificate and private key, or leave both empty."})}${configField("TLS private key file","server.private_key_file",{code:true,help:"Path only. The private key itself is never stored in browser view state."})}</div>`; }
  function limitsFields() { return `<div class="field-grid">${configField("Maximum connections","limits.max_connections",{type:"number",min:1,max:64,help:"Simultaneous ACP client connections; 1–64."})}${configField("Maximum message size · bytes","limits.max_frame_bytes",{type:"number",min:1024,max:16777216,help:"Maximum ACP frame size, from 1 KiB to 16 MiB."})}${configField("Backend startup timeout · seconds","limits.backend_start_timeout_seconds",{type:"number",min:1,max:300})}${configField("Idle timeout · seconds","limits.idle_timeout_seconds",{type:"number",min:30,max:86400})}${configField("Retained events","limits.event_retention",{type:"number",min:100,max:100000,help:"The number of recent events available for observation."})}</div>`; }
  function configFooter(action="config.save",label="Save configuration") { return `<div class="form-footer"><span><span class="dirty ${hasDrafts()?"visible":""}" data-dirty>Unsaved changes</span> <span class="small muted">Revision ${app.doc.revision}</span></span><div class="right">${button("Discard draft","draft.discard",{quiet:true})}${button(label,action,{primary:true,icon:"check"})}</div></div>`; }
  function installationDetails() {
    return `<dl class="detail-list installation-details"><div class="detail-row"><dt>Installation owner</dt><dd data-control="installation.owner">${esc(app.doc.config.owner)} <span class="readonly-badge">Fixed at installation</span></dd></div><div class="detail-row"><dt>Instance ID</dt><dd class="mono" data-control="installation.id">${esc(app.doc.config.instance_id)}</dd></div></dl>`;
  }
  function advancedSettings() {
    const expanded=state().filters?.advanced===true;
    return `<section class="card advanced-card"><button type="button" class="advanced-toggle" data-action="advanced.toggle" data-control="settings.advanced" aria-expanded="${expanded}" aria-controls="advanced-settings">${icon("settings")}<span><strong>Advanced settings</strong><small>Runtime, service access, and operating limits</small></span>${icon("chevron")}</button>${expanded?`<div id="advanced-settings" class="advanced-content"><div class="section-links">${["backend","limits","service"].map(s=>button(title(s),"section.focus",{small:true,attrs:`data-target="section.${s}"`})).join("")}</div><section data-control="section.backend" tabindex="-1"><h2>Runtime</h2><p class="small muted">The installed runtime is already configured. Change these settings to use a different ACP-compatible agent.</p>${backendFields()}<h3>Connected tools · optional</h3><p class="small muted">Tools belong to this agent across experiences. Only clients explicitly granted “Use connected tools” receive them. Store credentials in Access &amp; credentials, and use their names below.</p>${configField("HTTP MCP connections · JSON array","tool_connections",{json:true,rows:6,default:[],help:'For example [{"name":"project-tools","url":"https://tools.example/mcp","header_secrets":{"Authorization":"project-tools-auth"}}]. Only secret names belong here, never credential values. Changes disconnect clients currently using these tools.'})}</section><section class="subsection" data-control="section.limits" tabindex="-1"><h2>Operating limits</h2>${limitsFields()}</section><section class="subsection" data-control="section.service" tabindex="-1"><div class="card-header"><div><h2>Service & access</h2><p>Listener changes take effect after a service restart.</p></div>${badge(app.status?.restart_required?"Restart required":"Settings active",app.status?.restart_required?"amber":"green")}</div><div class="notice-strip"><p>Open this agent at ${esc((app.status?.addresses?.urls||[base.href]).join(" · "))}</p></div>${serverFields()}${certificateHelp()}</section></div>`:""}</section>`;
  }
  function certificateHelp() {
    const addresses=app.status?.addresses||{};
    const url=[...(addresses.welcome_urls||[]),addresses.welcome_url].find(u=>{try{return new URL(u).hostname===location.hostname;}catch{return false;}});
    return `<p class="card-note"><a href="welcome" data-control="certificate.help">Browser certificate help</a> · Steps for trusting this agent on Windows, Mac, or Linux.${url&&safeLink(url)?`<br>The guide can also open without HTTPS at <a href="${esc(safeLink(url))}" target="_blank" rel="noopener noreferrer">${esc(url)}</a>. Sign-in stays on the secure agent page.`:""}</p>`;
  }
  function accountLink(url) {try{const u=new URL(url);return u.protocol==="https:"&&u.hostname==="auth.openai.com"&&u.pathname==="/codex/device"?u.href:null;}catch{return null;}}
  function runtimeSummary() {
    const b=app.backend;
    if(!b)return `<section class="runtime-summary" data-control="runtime.summary"><h3>Checking the installed runtime…</h3><p class="small muted">The service is checking what this agent is ready to use.</p></section>`;
    const codex=b.kind==="codex", native=b.kind==="native", connected=b.auth_ready===true;
    let label=b.status==="custom"?"Custom runtime configured":b.installed?`${b.name||"Runtime"} installed`:b.configured?`${b.name||"Runtime"} needs attention`:"Runtime setup needed";
    let description=codex&&connected?(b.account_source==="owner_account"?"Using your existing ChatGPT account in this agent’s private account store.":"Your ChatGPT account is connected for this agent."):codex&&b.installed?"Connect your ChatGPT account so this agent can do work.":b.status==="custom"?"A custom runtime is configured for this agent.":b.help||"Install the included runtime to get this agent ready.";
    const actions=b.actions||[], pending=app.account?.state==="pending", link=pending?accountLink(app.account.verification_url):null;
    return `<section class="runtime-summary" data-control="runtime.summary"><div class="runtime-summary-head"><div class="runtime-icon">${icon(b.installed||b.status==="custom"?"check":"info")}</div><div><h3>${esc(label)}</h3><p>${esc(description)}</p></div>${badge(native?(connected?b.auth_mode==="chatgpt"?"ChatGPT account configured":"API key configured":b.status==="needs_key"?"API key needed":"Setup needed"):connected?"Account connected":b.status==="custom"?"Configured":b.installed?"Account needed":"Setup needed",connected||b.status==="custom"?"green":"amber")}</div>${b.problem?`<p class="runtime-problem">${esc(b.problem)}</p>`:""}${pending?`<div class="account-connect" data-control="account.pending"><h3>Finish connecting your account</h3><p>Open the secure sign-in page and enter this one-time code. This page will update when you’re connected.</p><div class="account-code">${esc(app.account.user_code||"Waiting for a code…")}</div><div class="row-actions">${link?`<a class="button primary" href="${esc(link)}" target="_blank" rel="noopener noreferrer" data-control="account.open">Open ChatGPT sign-in ${icon("arrow")}</a>`:""}${button("Cancel","backend.account.cancel",{quiet:true})}</div></div>`:app.account?.state==="failed"?banner(app.account.message||"The sign-in did not finish. Try connecting again.","warning"):""}<div class="row-actions">${actions.includes("install")?button(app.backendBusy?"Installing…":"Install runtime","backend.install",{primary:true,icon:"plus",disabled:app.backendBusy}):""}${actions.includes("connect_account")&&!pending?button(app.backendBusy?"Opening sign-in…":connected?"Reconnect account":"Connect ChatGPT","backend.account.connect",{primary:true,icon:"access",disabled:app.backendBusy}):""}</div>${codex&&connected?'<p class="runtime-note">Account details are ready. The first task will verify model access.</p>':b.help?`<p class="runtime-note">${esc(b.help)}</p>`:""}</section>`;
  }
  function setupProblem(config) {
    if(!config?.backend)return "Install a runtime to continue, or choose your own under Advanced settings.";
    const unchanged=config.backend.command===app.doc.config.backend?.command;
    if(unchanged&&app.backend?.kind==="codex"){
      if(!app.backend.installed)return "The included runtime needs to be installed before starting.";
      if(!app.backend.auth_ready)return "Connect your ChatGPT account before starting.";
    }
    return "";
  }
  function setup() {
    const steps=["Identity","Ready"];const step=Math.max(0,Math.min(1,Number(state().step)||0));
    let preview=null,validation="";try{preview=buildConfig();}catch(e){validation=e.message;}
    const problem=validation||setupProblem(preview);
    const content=step===0?`<h2 class="wizard-title">Give your agent a name and a purpose.</h2><p class="wizard-description">Tell it what you want help with and how you like to work. You can adjust these later.</p>${identityFields()}`:`<h2 class="wizard-title">Ready to make it yours.</h2><p class="wizard-description">Start the agent, then connect it to the conversations and tools you use.</p><dl class="detail-list"><div class="detail-row"><dt>Name</dt><dd>${esc(preview?.name||draft("config.name",app.doc.config.name))}</dd></div><div class="detail-row"><dt>Purpose</dt><dd>${esc(preview?.purpose||"You can add a purpose later.")}</dd></div></dl>${runtimeSummary()}${nativeConfigCard()}${nativeAccountCard()}${problem?banner(problem,"warning"):""}<p class="small muted">You’ll land on a dashboard with its connections, recent work, and settings.</p>${installationDetails()}`;
    return heading("A new individual starts here.","Name your agent. Give it a purpose. Start when you’re ready.")+`<div class="wizard"><nav class="wizard-steps" aria-label="Setup steps">${steps.map((label,i)=>`<button type="button" class="step-button ${i===step?"active":""}" data-action="setup.step" data-step="${i}" data-control="setup.step.${i}" ${i===step?'aria-current="step"':""}><span class="step-number">${i+1}</span>${label}</button>`).join("")}</nav><div><section class="card">${content}<div class="form-footer"><div>${step?button("Back","setup.step",{quiet:true,attrs:'data-step="0"'}):button("Save for later","config.save",{quiet:true})}</div><div class="right">${step===0?button("Continue","setup.next",{primary:true,icon:"arrow",attrs:'data-step="1"'}):button("Start my agent","setup.finish",{primary:true,icon:"play",disabled:!!problem||!app.backend||app.backendBusy})}</div></div></section>${advancedSettings()}</div></div>`;
  }
  function nativeConfigCard(){
    if(app.backend?.kind!=="native")return "";
    const n=app.nativeConfig;
    if(!n)return `<section class="card" data-control="native.config"><div class="card-header"><div><h2>Model & capabilities</h2><p>Configure this installation’s native engine.</p></div>${button("Open engine settings","native.load",{small:true})}</div>${app.nativeError?banner(app.nativeError,"warning"):""}</section>`;
    const c=n.config,presets=app.nativeSchema?.["x-agent-runtime"]?.provider_presets;
    const kinds=Array.isArray(presets)?presets.map(p=>[p.kind||p.provider?.kind||p.id,p.label||p.name||p.kind||p.id]).filter(p=>p[0]):Object.keys(presets||{}).map(k=>[k,title(k)]);
    if(!kinds.some(p=>p[0]===c.provider.kind))kinds.push([c.provider.kind,title(c.provider.kind)]);
    const native=c.provider.native||{};
    return `<section class="card" data-control="native.config"><div class="card-header"><div><h2>Model & capabilities</h2><p>Changes apply when clients reconnect. Background work requires renewed approval after changes.</p></div>${button("Add API key","native.key.open",{small:true})}${button("Reload","native.load",{small:true})}</div>${app.nativeError?banner(app.nativeError,"warning"):""}<div class="form-grid">${choice("Provider","native.kind",c.provider.kind,kinds)}${draft("native.auth",c.provider.auth||"auto")==="chatgpt"&&app.nativeModels?.length?choice("Model","native.model",c.provider.model,app.nativeModels.map(m=>[m.slug,m.display_name||m.slug])):field("Model","native.model",c.provider.model)}${choice("Authentication","native.auth",c.provider.auth||"auto",[[c.provider.auth||"auto",c.provider.auth==="chatgpt"?"ChatGPT account":"Provider credentials"],...(c.provider.auth==="chatgpt"?[["auto","Provider credentials"]]:[["chatgpt","ChatGPT account"]])])}${field("Service endpoint","native.endpoint",c.provider.endpoint)}${field("Credential environment name","native.key_env",c.provider.api_key_env,{help:"Reference a named secret in Advanced settings → Secret environment references. Never paste a key here."})}${choice("Default execution mode","native.mode",c.mode,[["ask","Ask before tool use"],["read_only","Read only"],["workspace","Workspace tools"]])}${Object.hasOwn(c,"command_network")?choice("Command network access","native.command_network",c.command_network,[["isolated","Off"],["inherit","Use installation network (Linux)"]]):""}</div><div class="row-actions">${button("Use provider defaults","native.preset",{small:true})}${checkbox("Accept images","native.images",native.image_input||false)}${checkbox("Accept audio","native.audio",native.audio_input||false)}${checkbox("Accept PDFs","native.pdf",native.pdf_input||false)}</div>${button(state().filters.native_details?"Hide tool and decision settings":"Tools & decision models","native.details",{small:true})}${state().filters.native_details?`<div class="form-grid">${field("Provider-native tools","native.tools",JSON.stringify(native.tools||[],null,2),{textarea:true,code:true,rows:6,help:"Use this provider’s tool definitions. The engine validates supported combinations before saving."})}${field("Computer environment (optional)","native.computer",JSON.stringify(native.computer||null,null,2),{textarea:true,code:true,rows:5,help:"Responses providers only. Bind an installed MCP server/tool and display dimensions. Each computer action batch requires approval; leave null to disable."})}${field("Decision services","native.decisions",JSON.stringify(c.decisions||[],null,2),{textarea:true,code:true,rows:6,help:"Separate named decision backends; credential names only."})}</div>`:""}<div class="form-footer">${button("Complete configuration…","native.raw.open",{quiet:true})}<div class="row-actions">${button("Check settings","native.validate",{small:true})}${button("Save engine settings","native.save",{primary:true})}</div></div><p class="card-note">Configuration validation does not call a model or establish account access. Stored revision ${esc(shortId(n.revision))}.</p></section>`;
  }
  async function loadNative(){try{const [n,s]=await Promise.all([api("native-config"),api("native-config/schema")]);app.nativeConfig=n;app.nativeSchema=s;app.nativeError="";await clearDraftNamespace("native");await loadNativeAccount(false);}catch(e){app.nativeError=e.message;}render();}
  function nativeDocument(){if(!app.nativeConfig)throw new Error("Load the engine settings first.");const c=clone(app.nativeConfig.config);c.provider.kind=String(draft("native.kind",c.provider.kind));c.provider.model=String(draft("native.model",c.provider.model));c.provider.endpoint=String(draft("native.endpoint",c.provider.endpoint));c.provider.api_key_env=String(draft("native.key_env",c.provider.api_key_env));c.provider.auth=String(draft("native.auth",c.provider.auth||"auto"));if(c.provider.auth==="chatgpt"){if(c.provider.kind!=="openai")throw new Error("ChatGPT account access uses the OpenAI provider. Choose Use this account below.");const file=String(draft("native.account_file",app.nativeAccount?.account?.credentials_file||c.provider.chatgpt_credentials_file||""));if(!file||!app.nativeAccount?.account?.connected)throw new Error("Connect this engine's ChatGPT account first.");c.provider.chatgpt_credentials_file=file;}else{delete c.provider.chatgpt_credentials_file;}c.mode=String(draft("native.mode",c.mode));if(Object.hasOwn(c,"command_network"))c.command_network=String(draft("native.command_network",c.command_network));c.provider.native??={};c.provider.native.image_input=!!draft("native.images",c.provider.native.image_input||false);c.provider.native.audio_input=!!draft("native.audio",c.provider.native.audio_input||false);c.provider.native.pdf_input=!!draft("native.pdf",c.provider.native.pdf_input||false);try{c.provider.native.computer=JSON.parse(String(draft("native.computer",JSON.stringify(c.provider.native.computer||null))));c.provider.native.tools=JSON.parse(String(draft("native.tools",JSON.stringify(c.provider.native.tools||[]))));c.decisions=JSON.parse(String(draft("native.decisions",JSON.stringify(c.decisions||[]))));}catch{throw new Error("Tools, decisions, and computer settings must be valid JSON.");}return c;}
  async function saveNative(validate=false,raw=false){await flushDrafts();let config;if(raw){try{config=JSON.parse(String(draft("native.raw","")));}catch{throw new Error("The complete configuration must be valid JSON.");}}else config=nativeDocument();const body={expected_revision:app.nativeConfig.revision,expected_backend_sha256:app.nativeConfig.backend_sha256,config};await api(validate?"native-config/validate":"native-config",{method:validate?"POST":"PUT",body});if(validate){setNotice("The installed engine accepts these settings. Model access still needs a real task.");}else{if(raw)await command({type:"close_modal"});await loadNative();await refresh(false);setNotice("Engine settings saved. Reconnect clients to use them; review background-work approval before the next run.");}}

  function nativeAccountCard(){
    if(app.backend?.kind!=="native")return "";
    const n=app.nativeAccount,job=n?.login||{},pending=["starting","pending","cancelling"].includes(job.state),connected=n?.account?.connected;
    const local=["localhost","127.0.0.1","[::1]"].includes(location.hostname),where=String(draft("account.location",local?"local":"remote"));
    const ready=where==="local"||state().filters.native_tunnel_ready;
    return `<section class="card" data-control="native.account"><div class="card-header"><div><h2>ChatGPT account</h2><p>Optional account access for this native engine. Connect here, then choose an available model and save the engine settings.</p></div>${button("Check account","native.account.load",{small:true})}</div>${app.nativeAccountError?banner(app.nativeAccountError,"warning"):""}${connected?`<p>${badge("Connected","green")} ${esc(n.account.email||"This installation’s account")}</p><p class="small muted">${n.account.sharing?"Plan access is enabled for this agent.":"Plan access is not enabled; account permission is needed before model calls."}</p><div class="row-actions">${button("Use this account","native.account.use",{primary:true})}${button("Load available models","native.account.models",{small:true})}${button("Disconnect account…","native.account.disconnect.open",{quiet:true})}</div>`:'<p class="small muted">This authorization belongs to the native engine. It does not reuse a Codex login or switch the current provider automatically.</p>'}${choice("Where is this browser?","account.location",local?"local":"remote",[["local","On the same machine as this agent"],["remote","On another computer"]])}${!pending?`<div class="row-actions">${button(connected?"Reconnect ChatGPT":"Connect ChatGPT","native.account.connect",{primary:!connected})}</div>`:`<div class="account-connect" data-control="native.account.pending"><h3>${job.state==="starting"?"Preparing sign-in…":job.state==="cancelling"?"Cancelling sign-in…":"Finish connecting your account"}</h3>${job.state==="pending"?`<p>Complete sign-in before ${esc(displayTime(job.expires_at))}. The callback is on the agent machine’s local connection.</p>${where==="remote"?`<p>First, open Terminal or PowerShell on this computer and run this connection command. Keep it open while signing in.</p>${job.tunnel_command?`<pre class="code-block">${esc(job.tunnel_command)}</pre>${button("Copy connection command","native.account.tunnel.copy",{small:true,icon:"copy"})}`:`<p>Ask an administrator to forward local port ${esc(job.callback_port)} to 127.0.0.1:${esc(job.callback_port)} on the agent machine.</p>`}<p class="small muted">SSH must already be available for your account. If the port is busy, cancel and start again. No public callback or account transfer is used.</p>${button(ready?"Connection marked ready":"I’ve started the connection","native.account.tunnel.ready",{small:true})}`:""}${ready?button("Open ChatGPT sign-in","native.account.open",{primary:true}):""}`:""}${button("Cancel sign-in","native.account.cancel",{quiet:true,disabled:job.state==="cancelling"})}</div>`}${!pending&&["failed","expired","interrupted","cancelled"].includes(job.state)?banner(job.message||({expired:"Sign-in expired. Start again when you are ready.",cancelled:"Sign-in was cancelled. Check the account above before starting again. Provider settings were preserved.",interrupted:"Sign-in was interrupted. Check the account before trying again."})[job.state]||"Sign-in did not finish.","warning"):""}<p class="card-note">Signing in alone does not change the model, reconnect clients, or renew background-work authorization.</p></section>`;
  }
  async function loadNativeAccount(redraw=true){try{app.nativeAccount=await api("native-account");app.nativeAccountError="";scheduleNativeAccountPoll();}catch(e){app.nativeAccountError=e.message;}if(redraw)render();}
  function scheduleNativeAccountPoll(){clearTimeout(app.nativeAccountTimer);if(!["starting","pending","cancelling"].includes(app.nativeAccount?.login?.state))return;app.nativeAccountTimer=setTimeout(async()=>{if(!app.session)return;try{app.nativeAccount.login=await api("native-account/login");if(app.nativeAccount.login.state==="connected"){await loadNativeAccount(false);app.backend=await api("backend");setNotice("ChatGPT account connected. Choose an available model and save engine settings to use it.");}else{render();scheduleNativeAccountPoll();}}catch(e){app.nativeAccountError=e.message;render();}},1000);}
  async function connectNativeAccount(){if(!app.nativeConfig)await loadNative();if(!app.nativeConfig)throw new Error("Open engine settings before connecting an account.");await flushDrafts();await command({type:"set_filter",key:"native_tunnel_ready",value:false});const port=Number(draft("account.callback_port",0));const job=await api("native-account/login",{method:"POST",body:{expected_backend_sha256:app.nativeConfig.backend_sha256,callback_port:port}});app.nativeAccount??={account:{connected:false}};app.nativeAccount.login=job;app.nativeAccountError="";scheduleNativeAccountPoll();render();}
  function nativeAuthorizationUrl(){const raw=app.nativeAccount?.login?.authorization_url;try{const u=new URL(raw);if(u.protocol==="https:"&&u.hostname==="auth.openai.com"&&u.pathname==="/api/accounts/authorize"&&!u.username&&!u.password)return u.href;}catch{}throw new Error("There is no active sign-in address. Check the account and start again.");}
  async function loadNativeModels(){const value=await api("native-account/models",{method:"POST",body:{}});app.nativeModels=value.models||[];if(!app.nativeModels.length)throw new Error("This account did not offer any models. Check its plan access before saving.");render();}
  async function useNativeAccount(){if(!app.nativeConfig)await loadNative();await loadNativeAccount(false);if(!app.nativeAccount?.account?.connected)throw new Error("Connect the account first.");await loadNativeModels();const choices=app.nativeModels,current=String(draft("native.model",app.nativeConfig.config.provider.model)),model=choices.some(m=>m.slug===current)?current:choices[0].slug;await patchView(next=>{next.draft.native={...(next.draft.native||{}),kind:"openai",endpoint:"https://api.openai.com/v1/responses",auth:"chatgpt",account_file:app.nativeAccount.account.credentials_file,model};});setNotice("The connected account and model are selected. Save engine settings to apply them.");}

  function configuration() {
    return heading("Make this agent yours.","Its purpose and working style can evolve as you work together.",button("Save changes","config.save",{primary:true,icon:"check"}))+(app.status?.config_revision!==undefined&&app.status.config_revision!==app.doc.revision?`<div class="notice-strip"><p>Settings changed outside this page. Your draft is preserved; load the latest settings before saving.</p>${button("Load latest settings","config.reload",{small:true,icon:"refresh"})}</div>`:"")+`<section class="card" data-control="section.identity" tabindex="-1"><div class="card-header"><div><h2>Identity & purpose</h2><p>The same individual, across clients and conversations.</p></div></div>${identityFields()}${installationDetails()}</section><section class="card"><h2>Ready to work</h2>${runtimeSummary()}</section>${nativeConfigCard()}${nativeAccountCard()}${advancedSettings()}${configFooter()}`;
  }
  function profiles() {
    const descriptions={communication:"How this agent communicates: tone, patterns, and the shape of its voice.","working-style":"How it approaches decisions, plans, tradeoffs, and getting work done.",identity:"Optional identity or personal context, kept separate from style and working guidance."};
    return heading("Context, with room to evolve.","Import a profile from Persona Workshop, or edit this agent’s guidance here.",button("Refresh","refresh",{icon:"refresh"}))+profilePackageCard()+profileUseCard()+Object.entries(profileNames).map(([key,label])=>`<section class="card profile-card"><div class="card-header"><div><h2>${esc(label)}</h2><p>${esc(descriptions[key])}</p></div>${badge(key==="identity"?"Separate personal context":"Profile input",key==="identity"?"amber":"")}</div>${field(label,`profiles.${key}`,app.profiles[key]||"",{textarea:true,long:true,help:key==="identity"?"Include only the personal details this agent needs for its intended role.":"Plain text or Markdown. Saving an edit creates a local profile version."})}<div class="form-footer"><span class="muted small">${esc(key)}.md</span><div class="row-actions">${button("Discard edits","profile.discard",{quiet:true,attrs:`data-profile="${key}"`})}${button("Save profile","profile.save",{primary:true,attrs:`data-profile="${key}"`,icon:"check"})}</div></div></section>`).join("");
  }
  function profileVersionLabel(version) {return version?.source?.revision?`Persona Workshop · ${shortId(version.source.revision)}`:version?.kind==="baseline"?"Before the first import":"Edited on this agent";}
  function profileHasDraft(key) {return String(draft(`profiles.${key}`,app.profiles[key]||""))!==String(app.profiles[key]||"");}
  function hasStyleDrafts() {return ["communication","working-style"].some(profileHasDraft);}
  function profilePackageCard() {
    const current=app.profilePackage?.active,previous=app.profilePackage?.previous||[],expanded=state().filters?.profile_package_history===true;
    return `<section class="card profile-package-card" data-control="profile.package.status" tabindex="-1"><div class="card-header"><div><h2>Installed profile</h2><p>${current?esc(profileVersionLabel(current)):"Bring this agent’s communication and working style over together."}</p></div>${button("Import profile package","profile.package.open",{primary:true,icon:"plus",disabled:app.profilePackageBusy,attrs:'data-control="profile.package.open"'})}</div>${app.profilePackageError?banner(app.profilePackageError,"warning"):current?`<div class="profile-package-status">${badge("Installed","green")}<span class="small muted">${esc(displayTime(app.profilePackage.applied_at||current.created_at))}</span></div>`:`<p class="small muted">No profile version installed yet. You can import a package or use the editors below.</p>`}<p class="card-note">Subsequent messages use the installed guidance. Work already in progress keeps its context, and earlier conversation history stays as it was.</p>${current?`<div class="profile-package-tools">${button(expanded?"Hide version history":"Version history","profile.package.history",{quiet:true,small:true,attrs:`data-control="profile.package.history" aria-expanded="${expanded}"`})}</div>${expanded?`<div class="profile-package-history" data-control="profile.package.versions"><dl class="detail-list"><div class="detail-row"><dt>Installed version</dt><dd class="mono">${esc(current.id)}</dd></div>${current.source?.revision?`<div class="detail-row"><dt>Source version</dt><dd class="mono">${esc(current.source.revision)}</dd></div>`:""}</dl>${previous.length?`<ul class="profile-version-list">${previous.map(version=>`<li><div><strong>${esc(profileVersionLabel(version))}</strong><span>${esc(displayTime(version.created_at))}</span></div>${button("Restore","profile.package.restore",{small:true,disabled:app.profilePackageBusy,attrs:`data-id="${esc(version.id)}" data-revision="${esc(app.profilePackage.revision)}" aria-label="Restore ${esc(profileVersionLabel(version))}"`})}</li>`).join("")}</ul>`:`<p class="small muted">Earlier versions will appear here after the next update.</p>`}</div>`:""}`:""}</section>`;
  }
  function profilePackageDialog() {
    const imported=app.profilePackageImport,dirty=hasStyleDrafts();
    return `<p class="small muted">Choose the package downloaded from Persona Workshop. It installs communication and working style together; identity and personal context stay separate.</p><label class="field"><span class="field-label">Profile package</span><input class="input profile-package-file" type="file" accept=".json,application/json" data-control="profile.package.file" ${app.profilePackageBusy?"disabled":""}><p class="field-help">A .persona.json or .twin.json export.</p></label>${imported?`<div class="profile-package-preview" data-control="profile.package.preview"><strong>Ready to install</strong><p>Source version <span class="mono">${esc(shortId(imported.manifest.revision))}</span></p><ul><li>Communication</li><li>Working style</li></ul></div>`:""}${app.profilePackageImportError?banner(app.profilePackageImportError,"error"):""}${app.profilePackageError?banner(app.profilePackageError,"warning"):""}${dirty?banner("Save or discard your communication and working-style edits before importing a package.","warning"):""}<div class="form-footer">${button("Cancel","modal.close",{quiet:true})}${button(app.profilePackageBusy?"Applying…":"Apply profile","profile.package.import",{primary:true,icon:"check",disabled:!imported||dirty||app.profilePackageBusy||!app.profilePackage||!!app.profilePackageError,attrs:'data-control="profile.package.apply"'})}</div>`;
  }
  function connectionName(c) { return c.client_name||c.client?.name||c.clientInfo?.name||c.name||c.id||c.connection_id||"ACP client"; }
  function sessionId(s) { return s.id||s.session_id||s.sessionId||""; }
  function contextSources() {
    const sources=app.contextProviders;
    return `<section class="card" data-control="context.sources"><div class="card-header"><div><h2>Conversation sources</h2><p>Places this same agent can consult when a conversation refers to work elsewhere.</p></div>${badge(`${sources.length} registered`,sources.length?"green":"")}</div>${app.contextError?banner(app.contextError,"warning"):sources.length?`<dl class="detail-list">${sources.map(source=>`<div class="detail-row"><dt>${esc(source.label)}</dt><dd>${badge(source.configured?"Credential configured":"Credential needed",source.configured?"green":"amber")}<div class="small muted">${esc(source.id)}</div></dd></div>`).join("")}</dl>`:empty("No conversation sources yet","Connect a conversation provider to let this agent retrieve context across experiences.","connections")}<p class="card-note">Access follows the current conversation’s audience. Sources are retrieved as needed; separate sessions retain their own working context. A configured credential does not guarantee the source is online.</p></section>`;
  }
  async function loadDevelopment(){try{app.development=await api("development");app.developmentError="";}catch(e){app.developmentError=e.message;}}
  function developmentCard(){
    const d=app.development,g=d?.github;
    return `<section class="card section-gap" data-control="development.setup"><div class="card-header"><div><h2>Work with code</h2><p>Connect GitHub so this agent can inspect repositories and propose changes.</p></div></div>${app.developmentError?banner(app.developmentError,"error"):""}
      ${!d?button("Check development setup","development.check",{primary:true}):g.configured?`<p>${g.verification?"GitHub account and development tools verified.":"GitHub is configured. Check its access before starting work."} ${g.local_clients_enabled?"Available to this installation’s local clients.":"Local clients still need permission to use connected tools."}</p><div class="actions">${button("Check access","development.verify",{small:true})}${button("Reconnect","development.open",{small:true,quiet:true})}${button("Disconnect GitHub","development.disconnect",{small:true,quiet:true})}</div>`:button("Connect GitHub","development.open",{primary:true})}
      ${d?`<p class="card-note">${esc(d.environment.message)}</p>`:""}</section>`;
  }
  function developmentDialog(){return `<p>Give this agent access to repositories and pull requests using the GitHub account signed in on this computer. Its access follows that account’s credential.</p><p>Local connections to this agent will be able to use these tools. Remote clients retain their existing permissions.</p><div class="actions">${button("Use signed-in GitHub account","development.connect",{primary:true})}</div>${button("Use a GitHub token instead","development.manual",{quiet:true,small:true})}${state().filters?.development_manual?`<label class="field"><span class="field-label">GitHub token</span><input class="input" type="password" autocomplete="off" data-sensitive data-control="development.credential" id="development-credential"><span class="field-help">Choose a token restricted to the repositories this agent should use. This value is never saved in display state.</span></label>${button("Connect token","development.connect.token",{primary:true})}`:""}<div class="form-footer">${button("Cancel","modal.close",{quiet:true})}</div>`;}
  function connections() {
    const connections=app.connections, sessions=app.sessions;
    return heading("One agent. Many connections.","ACP clients attach here. Each session carries its own working context.",button("Refresh","refresh",{icon:"refresh"}))+developmentCard()+contextSources()+`<section class="card"><div class="card-header"><div><h2>Connect a client</h2><p>Use this agent’s entrypoint from a compatible client or a separate adapter.</p></div>${badge("ACP","blue")}</div><div class="connection-code">${esc(attachInfo())}</div>${button("Copy connection information","connection.copy",{small:true,icon:"copy"})}</section><section class="card"><div class="card-header"><div><h2>Attached clients</h2><p>Negotiated connections reported by this service.</p></div>${badge(`${connections.length} connected`,connections.length?"green":"")}</div>${connections.length?`<div class="table-wrap"><table class="table"><thead><tr><th>Client</th><th>Transport</th><th>Connected</th><th>State</th></tr></thead><tbody>${connections.map(c=>`<tr><td><strong>${esc(connectionName(c))}</strong><div class="dim mono">${esc(shortId(c.id||c.connection_id))}</div></td><td>${esc(c.transport||"ACP")}</td><td>${esc(displayTime(c.connected_at||c.created_at||c.started_at))}</td><td>${badge(c.state||c.status||"Connected","green")}</td></tr>`).join("")}</tbody></table></div>`:empty("No clients attached yet","Connect through the ACP entrypoint above. Connections will appear here.")}</section><section class="card"><div class="card-header"><div><h2>Sessions</h2><p>Select a working context to inspect its reported state.</p></div>${badge(`${sessions.length} sessions`)}</div>${sessions.length?`<div class="table-wrap"><table class="table"><thead><tr><th>Session</th><th>Connection</th><th>Working directory</th><th>State</th></tr></thead><tbody>${sessions.map(s=>`<tr class="${state().selected_session===sessionId(s)?"selected":""}"><td><button type="button" class="select-row" data-action="session.select" data-session="${esc(sessionId(s))}">${esc(s.title||shortId(sessionId(s)))}</button></td><td class="mono">${esc(shortId(s.last_connection_id||s.connection_id||s.connectionId||s.client_id))}</td><td class="mono">${esc(s.cwd||s.working_directory||"—")}</td><td>${badge(s.state||s.status||"Available")}</td></tr>`).join("")}</tbody></table></div>`:empty("No working contexts yet","Sessions appear when an ACP client creates or loads one.","profile")}</section>${state().selected_session?`<section class="card" data-control="session.detail" tabindex="-1"><div class="card-header"><div><h2>Session detail</h2><p>${esc(state().selected_session)}</p></div>${button("Clear selection","session.clear",{quiet:true,small:true})}</div><pre class="code-block">${esc(app.sessionDetail?JSON.stringify(app.sessionDetail,null,2):"Loading session…")}</pre></section>`:""}`;
  }
  function activityCategory(filters=state().filters||{}) {return filters.problems_only?"problems":filters.category==="diagnostics"&&filters.diagnostics!==true?"all":filters.category||"all";}
  function activityFilterKey(filters=state().filters||{}) {return JSON.stringify({search:String(filters.search||""),category:activityCategory(filters),diagnostics:filters.diagnostics===true});}
  function filterCheck(label,key,control) {return `<label class="check"><input type="checkbox" data-action="filter" data-filter="${key}" data-control="${control}" ${state().filters?.[key]===true?"checked":""}>${esc(label)}</label>`;}
  function activity() {
    const filters=state().filters||{},diagnostics=filters.diagnostics===true;
    const categories={all:"All activity",work:"Work in progress",outcomes:"Outcomes",problems:"Problems",connections:"Connections",configuration:"Settings",access:"Access",runtime:"Agent state",...(diagnostics?{diagnostics:"Diagnostics"}:{})};
    const items=app.activity;
    return heading("Work, outcomes, and what needs attention.","Follow what your agent is doing, what it finished, and anything that needs your help.",button("Refresh","refresh",{icon:"refresh"}))+`<section class="card"><div class="filters"><input class="input" aria-label="Search activity" data-action="filter" data-filter="search" data-control="activity.search" placeholder="Search activity…" value="${esc(filters.search||"")}"><select class="input" aria-label="Activity category" data-action="filter" data-filter="category" data-control="activity.category" ${filters.problems_only?"disabled":""}>${Object.entries(categories).map(([key,label])=>`<option value="${key}" ${activityCategory({...filters,problems_only:false})===key?"selected":""}>${label}</option>`).join("")}</select><span class="small muted" aria-live="polite">${app.activityLoading?"Loading…":`${items.length} shown`}</span></div><div class="activity-options">${filterCheck("Only problems","problems_only","activity.problems")}${filterCheck("Include diagnostics","diagnostics","activity.diagnostics")}</div>${app.activityError?banner(app.activityError,"error"):""}<div data-control="activity.results">${items.length?`<ol class="activity-list">${items.map(e=>`<li class="activity-entry ${e.category==="problems"||e.severity==="error"?"has-problem":""}"><div class="activity-symbol">${icon(e.category==="outcomes"?"check":e.category==="problems"||e.severity==="error"?"info":"activity")}</div><div class="activity-body"><button class="event-button" type="button" data-action="event.inspect" data-event="${esc(e.seq)}"><strong>${esc(e.summary||title(e.kind))}</strong></button><div class="activity-meta"><span>${esc(displayTime(e.at))}</span><span>${esc(categories[e.category]||title(e.category))}</span>${e.data?.session_id?`<span>Session ${esc(shortId(e.data.session_id))}</span>`:""}${diagnostics?`<span class="mono">${esc(e.kind)}</span>`:""}</div></div>${e.category==="problems"||e.severity==="error"?badge("Needs attention","amber"):""}</li>`).join("")}</ol>`:empty(app.activityLoading?"Loading recent activity…":"No matching activity",filters.problems_only?"There are no problems in this view.":"Try a different filter, or check back after your agent has done some work.","activity")}</div>${app.activityHasMore?`<div class="activity-more">${button("Load earlier activity","activity.more",{disabled:app.activityLoading})}</div>`:""}<p class="card-note">${diagnostics?"Diagnostics includes interface updates and protocol details for troubleshooting.":"Interface updates and protocol details stay out of this view. Turn on diagnostics when you want to troubleshoot."}</p></section>`;
  }
  function secretName(s) {return typeof s==="string"?s:s.name;}
  function access() {
    return heading("Access, kept intentional.","Store named secrets and issue scoped access for clients, tools, and other agents.")+`<div class="grid two"><section class="card"><div class="card-header"><div><h2>Named secrets</h2><p>Values stay private. Reference a name in backend settings.</p></div>${button("Add secret","secret.new",{small:true,icon:"plus"})}</div>${app.secrets.length?app.secrets.map(s=>`<div class="secret-row"><div><strong class="mono">${esc(secretName(s))}</strong><div class="token-meta">Value is write-only</div></div><div class="row-actions">${button("Replace","secret.edit",{small:true,quiet:true,attrs:`data-name="${esc(secretName(s))}"`})}${button("Remove","secret.remove",{small:true,quiet:true,attrs:`data-name="${esc(secretName(s))}"`})}</div></div>`).join(""):empty("No stored secrets","Add a secret only when your backend needs it.","access")}</section><section class="card"><div class="card-header"><div><h2>Sign-in & view control</h2><p>This installation is owned by your machine account.</p></div></div><dl class="detail-list"><div class="detail-row"><dt>Owner</dt><dd>${esc(app.doc.config.owner)}</dd></div><div class="detail-row"><dt>Sign-in provider</dt><dd>${app.status?.authentication==="windows_logon"?"Windows account":`PAM · ${esc(app.doc.config.server.pam_service)}`}</dd></div><div class="detail-row"><dt>This display</dt><dd class="mono">${esc(app.view?.id)}</dd></div><div class="detail-row"><dt>View revision</dt><dd>${esc(app.view?.revision||0)}</dd></div></dl><p class="card-note">A token with UI access can control a selected browser’s application view. It cannot read passwords or secret input values.</p>${button("Inspect view state","view.inspect",{small:true,icon:"screen"})}${certificateHelp()}</section></div><section class="card section-gap"><div class="card-header"><div><h2>Access tokens</h2><p>Use scoped tokens for automation instead of sharing your machine password.</p></div>${button("Create token","token.new",{primary:true,small:true,icon:"plus"})}</div>${app.tokens.length?app.tokens.map(t=>`<div class="secret-row"><div><strong>${esc(t.name||shortId(t.id))}</strong><div class="token-meta">${esc((t.scopes||[]).join(" · "))} · Expires ${esc(displayTime(t.expires_at||t.expires))}</div></div>${button("Revoke","token.revoke",{small:true,danger:true,attrs:`data-token="${esc(t.id)}"`})}</div>`).join(""):empty("No automation tokens","Create only the access a particular client or task needs.","access")}<p class="card-note">Observe reads service state. UI adds view control. ACP connects a working client. Owner context additionally gives that client access to the owner’s conversations and responsibility tools. Grant it only to private owner experiences. Profiles permits profile-package updates. Admin permits configuration and runtime changes.</p></section>`;
  }

  function modal() {
    const name=state().modal; if(!name)return ""; let body="",headingText="",wide=false;
    if(name==="profile-package"){headingText="Import a profile";body=profilePackageDialog();}
    else if(name==="secret"){headingText="Store a named secret";body=`${field("Secret name","access.secret_name","",{placeholder:"provider-key",help:"Letters, numbers, hyphens, and underscores. This name can appear in configuration."})}<label class="field"><span class="field-label">Secret value</span><textarea class="input code" data-sensitive data-secret-input data-control="access.secret_value" rows="3" autocomplete="off" spellcheck="false" placeholder="Enter a value to store or replace"></textarea><p class="field-help">Sent directly to the secret store. It is never saved as a view draft or shown again.</p></label><div class="form-footer"><span></span>${button("Store secret","secret.save",{primary:true,icon:"check"})}</div>`;}
    else if(name==="development-github"){headingText="Connect GitHub";body=developmentDialog();}
    else if(name==="native-account-disconnect"){headingText="Disconnect this ChatGPT account?";body=`<p>This revokes the native engine’s saved account access. Its conversations and account binding remain. Existing work that depends on this account may fail.</p><div class="form-footer">${button("Keep connected","modal.close",{quiet:true})}${button("Disconnect account","native.account.disconnect",{danger:true})}</div>`;}
    else if(name==="native-key"){headingText="Connect your model provider";body=`<p>Paste an API key for the provider selected in Model & capabilities.</p><label class="field"><span class="field-label">API key</span><input class="input" type="password" data-sensitive data-secret-input data-control="native.key_value" autocomplete="off"><span class="field-help">Stored privately for this agent. It is never saved as a view draft or displayed again.</span></label><div class="form-footer">${button("Cancel","modal.close",{quiet:true})}${button("Save API key","native.key.save",{primary:true})}</div>`;}
    else if(name==="native-raw"){headingText="Complete native engine configuration";wide=true;body=field("Configuration","native.raw","",{textarea:true,code:true,rows:20,help:"All settings supported by this installed engine version. Credential values belong in Access & credentials."})+`<div class="form-footer">${button("Cancel","modal.close",{quiet:true})}${button("Save configuration","native.raw.save",{primary:true})}</div>`;}
    else if(name.startsWith("memory-")){headingText=name==="memory-new"?"Remember something":"Update memory";body=memoryDialog();}
    else if(name==="scheduler"){headingText="Background work settings";body=schedulerDialog();}
    else if(name==="completion"){headingText="Acceptance and review";body=completionDialog();}
    else if(name==="execution"){headingText="Authorize or review a run";body=executionDialog();}
    else if(name.startsWith("work-")){headingText=name==="work-new"?"Add responsibility":name==="work-import"?"Receive delegation":"Update responsibility";body=workDialog(name);}
    else if(name.startsWith("organization-")){headingText="Configure "+name.slice(13);body=organizationDialog(name.slice(13));}
    else if(name==="token"){headingText="Create scoped access";body=`${field("Token name","access.token_name","",{placeholder:"My ACP client"})}<div class="field"><span class="field-label">Permissions</span><div class="scope-grid">${["observe","ui","acp","context:owner","tools:use","profiles","work:read","work","work:execute","memory:read","memory","admin"].map(s=>checkbox(({observe:"Observe service state",ui:"Control browser views",acp:"Connect through ACP","context:owner":"Use owner context and responsibilities","tools:use":"Use connected tools",profiles:"Install and restore profiles","work:read":"Read responsibilities and relationships",work:"Manage responsibilities and prepare sharing","memory:read":"Read lasting knowledge",memory:"Record and resolve lasting knowledge","work:execute":"Authorize and control background execution",admin:"Administer this instance"})[s],`access.token_scopes.${s}`,s==="observe")).join("")}</div></div>${field("Expires after · hours","access.token_hours",24,{type:"number",min:1,max:8760,help:"The token is displayed only once. Revoke it any time from this page."})}<div class="form-footer"><span></span>${button("Create token","token.create",{primary:true,icon:"plus"})}</div>`;}
    else if(name==="token-created"){headingText="Your token is ready";body=app.tokenOnce?`${banner("Copy this token now. Its value will not be shown again.","warning")}<label class="field"><span class="field-label">New token</span><textarea class="input code" data-sensitive data-token-once readonly rows="4" data-control="access.new_token"></textarea></label><div class="form-footer">${button("Copy token","token.copy",{icon:"copy"})}${button("I’ve saved it","modal.close",{primary:true})}</div>`:banner("The token value is no longer in this browser. Create another token if you did not save it.","warning");}
    else if(name==="view"){headingText="This browser’s view";wide=true;body=`<p class="small muted">This state can be inspected and controlled through the management API. It contains no password, secret, or token values.</p><pre class="code-block">${esc(JSON.stringify(app.view,null,2))}</pre>`;}
    else if(String(name).startsWith("event:")){headingText="Event detail";wide=true;const event=[...app.activity,...app.activityRecent,...app.events].find(e=>String(e.seq)===String(name).slice(6));body=`<pre class="code-block">${esc(event?JSON.stringify(event,null,2):"This event is no longer in the current observation window.")}</pre>`;}
    else return "";
    return `<div class="modal-backdrop" data-modal-backdrop><section class="modal ${wide?"wide":""}" role="dialog" aria-modal="true" aria-labelledby="modal-title"><div class="modal-header"><h2 id="modal-title">${esc(headingText)}</h2>${button("","modal.close",{quiet:true,icon:"close",attrs:'aria-label="Close dialog"'})}</div>${body}</section></div>`;
  }

  function captureDom() {
    const active=document.activeElement;const control=active?.getAttribute?.("data-control") || active?.id;
    const sensitive=[...root.querySelectorAll("[data-sensitive]")].map(el=>({key:el.getAttribute("data-control"),value:el.value}));
    return {control,start:active?.selectionStart,end:active?.selectionEnd,sensitive,scroll:{x:window.scrollX,y:window.scrollY}};
  }
  function findControl(id) { if(!id)return null; return [...root.querySelectorAll("[data-control]")].find(el=>el.dataset.control===id) || document.getElementById(id); }
  function render() {
    if (app.doc?.config.name) document.title = `${app.doc.config.name}: Agent Instance`;
    if(app.loading)return;
    const saved=captureDom();
    if(!app.session){root.innerHTML=loginScreen();restoreSensitive(saved);return;}
    const page={setup,dashboard,work:workPage,memory:memoryPage,organization:organizationPage,configuration,profiles,connections,activity,access}[currentRoute()]||dashboard;
    root.innerHTML=shell(page());
    restoreSensitive(saved);
    if(app.tokenOnce){const field=root.querySelector("[data-token-once]");if(field)field.value=app.tokenOnce;}
    const target=findControl(app.focusRemote || saved.control);
    app.renderError=app.focusRemote&&!target?`No visible control named ${String(app.focusRemote).slice(0,100)}`:null;
    if(target){target.focus({preventScroll:true});if(!app.focusRemote && saved.start!=null && target.setSelectionRange){try{target.setSelectionRange(saved.start,saved.end);}catch{}}}
    if(app.scrollRemote){window.scrollTo(app.scrollRemote.x||0,app.scrollRemote.y||0);app.scrollRemote=null;}else window.scrollTo(saved.scroll.x,saved.scroll.y);
    app.focusRemote=null;
    if(app.view){requestAnimationFrame(()=>requestAnimationFrame(()=>ackView()));}
  }
  function restoreSensitive(saved) { for(const x of saved.sensitive){if(!x.key)continue;const el=findControl(x.key);if(el?.hasAttribute("data-sensitive"))el.value=x.value;} }
  async function ackView() {
    const v=app.view;if(!v || v.revision===app.lastAck || !app.session)return;
    app.lastAck=v.revision;
    try{await api(`views/${encodeURIComponent(v.id)}/ack`,{method:"POST",body:{revision:v.revision,status:app.renderError?"error":"applied",detail:app.renderError||(document.visibilityState==="visible"?"View rendered in visible browser":"View rendered in background browser")}});}catch(e){app.lastAck=0;}
  }
  function hasDrafts() { const d=state().draft||{};return [...app.draftPending.keys()].some(k=>k.startsWith("config.")) || Object.keys(d.config||{}).length>0 || Object.keys(d).some(k=>k.startsWith("config.")); }
  function markDirty() { root.querySelectorAll("[data-dirty]").forEach(el=>el.classList.toggle("visible",hasDrafts())); }

  async function applyView(view,remote=false) {
    if(!view?.state || (app.view && view.id!==app.view.id))return;
    if(app.view && Number(view.revision)<Number(app.view.revision))return;
    const before=app.view?.state||{};app.view=view;
    if(remote && before.focus!==view.state.focus)app.focusRemote=typeof view.state.focus==="object"?view.state.focus?.target:view.state.focus;
    if(remote && JSON.stringify(before.scroll)!==JSON.stringify(view.state.scroll))app.scrollRemote=view.state.scroll;
    if(before.route!==view.state.route)app.scrollRemote={x:0,y:0};
    if(before.modal!==view.state.modal && view.state.modal!=="token-created")app.tokenOnce=null;
    if(before.modal==="profile-package" && view.state.modal!=="profile-package"){app.profilePackageImport=null;app.profilePackageImportError="";app.profilePackageExpected=null;}
    render();
    if(before.selected_session!==view.state.selected_session)await loadSessionDetail();
    if(view.state.route==="activity"&&(before.route!=="activity"||activityFilterKey(before.filters)!==activityFilterKey(view.state.filters)))await loadActivity();
    if(view.state.route==="profiles" && before.route!=="profiles")await loadProfiles();
    if(view.state.route==="memory"&&before.route!=="memory")await loadMemory();
    if(["work","organization"].includes(view.state.route)&&(before.route!==view.state.route||before.filters?.work_id!==view.state.filters?.work_id))await loadWork();
  }
  function command(commandValue) {
    if(commandValue?.type==="set_draft"){
      const path=String(commandValue.path||"");
      const allowed=Object.keys(configFields).map(k=>`config.${k}`).concat(Object.keys(profileNames).map(k=>`profiles.${k}`),workDrafts.concat(["access.secret_name","access.token_name","access.token_hours","access.token_scopes.observe","access.token_scopes.ui","access.token_scopes.acp","access.token_scopes.context:owner","access.token_scopes.tools:use","access.token_scopes.profiles","access.token_scopes.admin","access.token_scopes.work:read","access.token_scopes.work","access.token_scopes.work:execute","access.token_scopes.memory:read","access.token_scopes.memory"]));
      if(!allowed.includes(path))return Promise.reject(new Error("Use a supported non-secret draft field. Credentials have dedicated write-only APIs."));
    }
    const work=async()=>{
      if(!app.view)throw new Error("This browser has no registered view yet.");
      for(let attempt=0;attempt<2;attempt++){
        try {const v=await api(`views/${encodeURIComponent(app.view.id)}/commands`,{method:"POST",body:{expected_revision:app.view.revision,command:commandValue}});await applyView(v,true);return clone(v);}
        catch(e){if(e.status===409 && attempt===0){app.view=await api(`views/${encodeURIComponent(app.view.id)}`);continue;}throw e;}
      }
    };
    const result=app.commandQueue.then(work,work);app.commandQueue=result.catch(()=>{});return result;
  }
  function queueDraft(key,value) {
    if(key.startsWith("readonly."))return;
    app.draftPending.set(key,value);markDirty();
    clearTimeout(app.timers.get(`draft:${key}`));
    app.timers.set(`draft:${key}`,setTimeout(()=>flushDraft(key).catch(fail),450));
  }
  async function flushDraft(key) {
    clearTimeout(app.timers.get(`draft:${key}`));app.timers.delete(`draft:${key}`);
    if(!app.draftPending.has(key))return;
    const value=app.draftPending.get(key);
    await command({type:"set_draft",path:key,value});
    if(app.draftPending.get(key)===value)app.draftPending.delete(key);
  }
  async function flushDrafts() {for(const key of [...app.draftPending.keys()])await flushDraft(key);await app.commandQueue;}
  function patchView(change) {
    const work=async()=>{
      for(let attempt=0;attempt<2;attempt++){
        const next=clone(state());change(next);
        try{const view=await api(`views/${encodeURIComponent(app.view.id)}`,{method:"PATCH",body:{expected_revision:app.view.revision,state:next}});await applyView(view,true);return view;}
        catch(e){if(e.status===409&&attempt===0){app.view=await api(`views/${encodeURIComponent(app.view.id)}`);continue;}throw e;}
      }
    };
    const result=app.commandQueue.then(work,work);app.commandQueue=result.catch(()=>{});return result;
  }
  async function clearDraftNamespace(namespace) {
    for(const key of [...app.draftPending.keys()])if(key===namespace || key.startsWith(`${namespace}.`))app.draftPending.delete(key);
    await patchView(next=>{next.draft=next.draft||{};delete next.draft[namespace];for(const key of Object.keys(next.draft))if(key.startsWith(`${namespace}.`))delete next.draft[key];});
  }

  const configFields={
    name:"string",purpose:"string",instructions:"string",tool_connections:"connections",
    "backend.command":"string","backend.args":"array","backend.env":"object","backend.secret_env":"object","backend.working_directory":"string","backend.allowed_working_roots":"array",
    "server.bind":"string","server.port":"integer","server.welcome_port":"nullable-integer","server.public_url":"nullable","server.pam_service":"string","server.embed_origins":"array","server.certificate_file":"nullable","server.private_key_file":"nullable",
    "limits.max_connections":"integer","limits.max_frame_bytes":"integer","limits.backend_start_timeout_seconds":"integer","limits.idle_timeout_seconds":"integer","limits.event_retention":"integer"
  };
  function buildConfig() {
    const c=clone(app.doc.config);c.tool_connections=c.tool_connections||[];c.backend=c.backend||{command:"",args:[],env:{},secret_env:{},working_directory:"${AGENT_HOME}/workspace",allowed_working_roots:[]};
    for(const [path,type] of Object.entries(configFields)){
      let v=draft(`config.${path}`,get(c,path));
      if(type==="array" || type==="object" || type==="connections"){
        if(typeof v==="string"){try{v=JSON.parse(v);}catch{throw new Error(`${title(path)} must be valid JSON.`);}}
        if(type==="connections" && v===undefined)v=[];
        if(type==="connections" && !Array.isArray(v))throw new Error("Tool connections must be a JSON array.");
        if(type==="array" && (!Array.isArray(v)||v.some(x=>typeof x!=="string")))throw new Error(`${title(path)} must be an array of strings.`);
        if(type==="object" && (!v || typeof v!=="object" || Array.isArray(v) || Object.values(v).some(x=>typeof x!=="string")))throw new Error(`${title(path)} must be an object with string values.`);
      }else if(type==="nullable-integer"){if(v===""||v==null)v=null;else if(!Number.isInteger(Number(v)))throw new Error(`${title(path)} must be a whole number or left empty.`);else v=Number(v);}
      else if(type==="integer"){if(v==="" || !Number.isInteger(Number(v)))throw new Error(`${title(path)} must be a whole number.`);v=Number(v);}
      else if(type==="nullable")v=String(v||"").trim()||null;
      else v=String(v??"");
      put(c,path,v);
    }
    if(!c.name.trim())throw new Error("Give the agent a name before saving.");
    if(!c.backend.command.trim())c.backend=null;
    return c;
  }
  async function saveConfig(finish=false) {
    await flushDrafts();const config=buildConfig();if(finish){config.setup_complete=true;const problem=setupProblem(config);if(problem)throw new Error(problem);}
    const result=await api("config",{method:"PUT",body:{expected_revision:app.doc.revision,config}});
    app.doc=result?.config?result:await api("config");await clearDraftNamespace("config");
    if(finish){await api("lifecycle",{method:"POST",body:{action:"start"}});app.doc=await api("config");await command({type:"navigate",route:"dashboard"});}
    await refresh(false);setNotice(finish?"Your agent is configured and enabled. Its ACP connection information is below.":"Configuration saved. Listener changes take effect after a service restart.");
  }
  async function loadSessionDetail() {
    const id=state().selected_session;if(!id){app.sessionDetail=null;return;}
    try{const detail=await api(`sessions/${encodeURIComponent(id)}`);if(state().selected_session===id){app.sessionDetail=detail;render();}}catch(e){app.sessionDetail={error:e.message};render();}
  }

  async function loadProfiles() {
    const keys=Object.keys(profileNames),results=await Promise.allSettled([api("profile-package"),...keys.map(key=>api(`profiles/${key}`)),api("profile-use")]);
    if(results[0].status==="fulfilled"){app.profilePackage=results[0].value;app.profilePackageError="";}
    else app.profilePackageError="The installed profile version could not be loaded. Refresh to try again.";
    keys.forEach((key,i)=>{if(results[i+1].status==="fulfilled")app.profiles[key]=results[i+1].value?.text||"";else app.profilePackageError="Some profile details could not be loaded. Refresh to try again.";});
    const use=results[keys.length+1];if(use.status==="fulfilled"){app.profileUse=use.value;app.profileUseError="";}else{app.profileUse=null;app.profileUseError="Profile delivery evidence could not be loaded.";}
    render();
  }
  async function discardProfileDraft(key,expectedText) {
    if(!Object.hasOwn(profileNames,key))throw new Error("Unknown profile.");
    const path=`profiles.${key}`;
    if(expectedText!==undefined&&String(draft(path,app.profiles[key]||""))!==expectedText)return;
    clearTimeout(app.timers.get(`draft:${path}`));app.timers.delete(`draft:${path}`);app.draftPending.delete(path);
    await patchView(next=>{const drafts=next.draft||{},current=Object.hasOwn(drafts,path)?drafts[path]:(get(drafts,path)??app.profiles[key]??"");if(expectedText!==undefined&&String(current)!==expectedText)return;if(next.draft?.profiles)delete next.draft.profiles[key];delete next.draft?.[path];});
  }
  async function prepareProfilePackage(value,expectedRevision) {
    app.profilePackageImport=null;app.profilePackageImportError="";
    try {
      const imported=typeof value==="string"?JSON.parse(value):clone(value);
      if(!imported?.manifest?.revision || !imported?.files || ["communication.md","working-style.md"].some(name=>typeof imported.files[name]!=="string"))throw new Error("Choose a profile package exported from Persona Workshop. It must contain both communication and working style.");
      app.profilePackageImport=imported;app.profilePackageExpected=expectedRevision??app.profilePackage?.revision;
      await command({type:"open_modal",modal:"profile-package"});render();
      return {ready:true,source_revision:imported.manifest.revision};
    } catch(e) {app.profilePackageImportError=e instanceof SyntaxError?"This file is not valid JSON. Choose the profile package downloaded from Persona Workshop.":e.message;render();throw new Error(app.profilePackageImportError);}
  }
  async function applyProfilePackage(payload={},restore=false) {
    if(app.profilePackageBusy)throw new Error("A profile update is already in progress.");
    await flushDrafts();
    if(hasStyleDrafts())throw new Error("Save or discard your communication and working-style edits before changing the installed profile.");
    const imported=payload.package??app.profilePackageImport;
    if(!restore&&!imported)throw new Error("Choose a profile package first.");
    if(!app.profilePackage)await loadProfiles();
    if(!app.profilePackage || app.profilePackageError)throw new Error("Refresh the installed profile before applying a change.");
    const expected=payload.expected_revision??(restore?payload.revision:app.profilePackageExpected)??app.profilePackage.revision;
    const expectedRevision=Number(expected);
    if(!Number.isInteger(expectedRevision)||expectedRevision<0)throw new Error("The profile version is missing. Refresh and try again.");
    app.profilePackageBusy=true;app.profilePackageImportError="";render();
    try {
      // Clear only the preflight's unchanged drafts before installing. Edits made
      // while the request is in flight remain visible for the user afterward.
      const unchangedProfiles={...app.profiles};
      await discardProfileDraft("communication",unchangedProfiles.communication||"");await discardProfileDraft("working-style",unchangedProfiles["working-style"]||"");
      const body=restore?{expected_revision:expectedRevision,id:payload.id}:{expected_revision:expectedRevision,package:imported};
      const result=await api(restore?"profile-package/restore":"profile-package",{method:"POST",body});
      app.profilePackage=result;
      await loadProfiles();
      if(state().modal==="profile-package")await command({type:"close_modal"});
      setNotice(restore?"Previous profile restored. Subsequent messages will use this version.":"Profile installed. Subsequent messages will use this version.");
      return result;
    } catch(e) {
      if(e.status===409){await loadProfiles();app.profilePackageExpected=app.profilePackage?.revision;e=new Error("This agent’s profile changed while you were reviewing it. The latest version is shown; review it and apply again if you still want this change.");}
      app.profilePackageImportError=e.message;render();throw e;
    } finally {app.profilePackageBusy=false;render();}
  }

  async function perform(action,payload={}) {
    switch(action){
      case "development.check":await loadDevelopment();render();return;
      case "development.open":await loadDevelopment();return command({type:"open_modal",modal:"development-github"});
      case "development.manual":return command({type:"set_filter",key:"development_manual",value:!state().filters?.development_manual});
      case "development.connect":
      case "development.connect.token":{
        const credential=action.endsWith(".token")?(payload.credential??document.getElementById("development-credential")?.value):undefined;
        if(action.endsWith(".token")&&!credential)throw new Error("Enter a GitHub token.");
        app.development=await api("development/github",{method:"POST",body:{expected_revision:app.development.config_revision,enable_local_clients:true,...(credential?{credential}:{})}});
        app.doc=await api("config");await command({type:"close_modal"});app.developmentError=app.development.local_clients_error||"";setNotice("GitHub connected. Reconnect active conversations to pick up its tools.");return;
      }
      case "development.verify":try{app.development=await api("development/github/check",{method:"POST",body:{}});app.developmentError="";setNotice("GitHub access verified. No repository was changed.");}catch(e){await loadDevelopment();throw e;}return;
      case "development.disconnect":await api("development/github",{method:"DELETE",body:{expected_revision:app.development.config_revision}});app.doc=await api("config");await loadDevelopment();setNotice("GitHub tools disconnected from this agent.");return;
      case "identity.avatar.choose":if(payload.data)return savePortrait(payload);return {action:"identity.avatar.upload",required:["media_type","data"],expected_revision:app.identity?.avatar_revision};
      case "identity.avatar.upload":return savePortrait(payload);
      case "identity.avatar.remove":app.identity=await api(`identity/avatar?expected_revision=${app.identity.avatar_revision}`,{method:"DELETE"});setNotice("Agent image removed. Connected experiences will use the default image.");return;
      case "native.account.load":return loadNativeAccount();
      case "native.account.connect":return connectNativeAccount();
      case "native.account.models":return loadNativeModels();
      case "native.account.use":return useNativeAccount();
      case "native.account.cancel":app.nativeAccount.login=await api("native-account/login",{method:"DELETE",body:{job_id:app.nativeAccount.login.id}});scheduleNativeAccountPoll();render();return;
      case "native.account.tunnel.copy":if(!app.nativeAccount?.login?.tunnel_command)throw new Error("The connection command is not ready.");await navigator.clipboard.writeText(app.nativeAccount.login.tunnel_command);setNotice("Connection command copied.");return;
      case "native.account.tunnel.ready":return command({type:"set_filter",key:"native_tunnel_ready",value:true});
      case "native.account.open":window.open(nativeAuthorizationUrl(),"_blank","noopener,noreferrer");return;
      case "native.account.disconnect.open":return command({type:"open_modal",modal:"native-account-disconnect"});
      case "native.account.disconnect":await api("native-account",{method:"DELETE",body:{confirm:true}});await command({type:"close_modal"});await loadNativeAccount();app.backend=await api("backend");setNotice("The native account has been disconnected. Engine settings and history were preserved.");return;
      case "native.key.open":if(!app.nativeConfig)await loadNative();return command({type:"open_modal",modal:"native-key"});
      case "native.key.save":{const value=payload.value??root.querySelector("[data-secret-input]")?.value;if(!value)throw new Error("Enter an API key.");await api("native-config/credential",{method:"POST",body:{expected_backend_sha256:app.nativeConfig.backend_sha256,value}});const input=root.querySelector("[data-secret-input]");if(input)input.value="";await command({type:"close_modal"});await refresh(false);await loadNative();setNotice("API key connected. The next task will verify model access.");return;}
      case "native.load":return loadNative();
      case "native.details":return command({type:"set_filter",key:"native_details",value:!state().filters.native_details});
      case "native.preset":{await flushDrafts();const kind=String(draft("native.kind",app.nativeConfig.config.provider.kind));const preset=app.nativeSchema?.["x-agent-runtime"]?.provider_presets?.find(p=>p.kind===kind);if(!preset)throw new Error("This engine did not publish defaults for that provider.");return patchView(next=>{next.draft.native={...(next.draft.native||{}),endpoint:preset.endpoint,key_env:preset.api_key_env,images:false,audio:false,pdf:false,computer:"null",tools:"[]"};});}
      case "native.save":return saveNative();
      case "native.validate":return saveNative(true);
      case "native.raw.open":await flushDrafts();return patchView(next=>{next.draft.native??={};next.draft.native.raw=JSON.stringify(nativeDocument(),null,2);next.modal="native-raw";});
      case "native.raw.save":return saveNative(false,true);
      case "memory.new":return openMemory(payload);
      case "memory.select":return command({type:"set_filter",key:"memory_id",value:payload.id||null});
      case "memory.archived":return command({type:"set_filter",key:"memory_archived",value:!state().filters.memory_archived});
      case "memory.action":{const record=app.memory?.items.find(r=>r.id===payload.id);if(!record)throw new Error("Memory no longer exists");return patchView(next=>{next.filters.memory_id=record.id;next.filters.memory_revision=record.revision;next.filters.memory_action=payload.command;next.filters.memory_candidate=payload.candidate||null;next.draft.memory={reason:""};next.modal="memory-command";});}
      case "memory.save":return saveMemory();
      case "scheduler.open":await loadWork();await patchView(next=>{next.draft.scheduler={enabled:app.scheduler.settings.enabled,paused:app.scheduler.settings.paused,concurrency:app.scheduler.settings.concurrency,approved:app.scheduler.settings.approved_backend_sha256===app.scheduler.backend_sha256,session_mode:app.scheduler.settings.session_mode||""};next.filters.scheduler_revision=app.scheduler.settings.revision;next.filters.scheduler_backend=app.scheduler.backend_sha256;next.modal="scheduler";});return;
      case "scheduler.advanced":return command({type:"set_filter",key:"scheduler_advanced",value:!state().filters.scheduler_advanced});
      case "scheduler.save":return saveScheduler();
      case "execution.report":return command({type:"set_filter",key:"execution_report",value:state().filters.execution_report===payload.id?null:payload.id});
      case "execution.limits":return command({type:"set_filter",key:"execution_limits",value:!state().filters.execution_limits});
      case "execution.open":return openExecution(payload);
      case "execution.save":return saveExecution();
      case "completion.open":return openCompletion(payload);
      case "completion.save":return saveCompletion();
      case "work.new":await loadWork();await clearDraftNamespace("work");return command({type:"open_modal",modal:"work-new"});
      case "work.select":return command({type:"set_filter",key:"work_id",value:payload.id||null});
      case "work.action":return workAction(payload);
      case "work.save":return saveWork();
      case "work.import.open":await loadWork();await clearDraftNamespace("work");return command({type:"open_modal",modal:"work-import"});
      case "organization.edit":return editOrganization(payload);
      case "organization.save":return saveOrganization(payload.kind);
      case "navigate":await flushDrafts();return command({type:"navigate",route:payload.route});
      case "setup.step":await flushDrafts();return command({type:"navigate",route:"setup",step:Math.max(0,Math.min(1,Number(payload.step)||0))});
      case "setup.next":await flushDrafts();buildConfig();return command({type:"navigate",route:"setup",step:1});
      case "advanced.toggle":return command({type:"set_filter",key:"advanced",value:state().filters?.advanced!==true});
      case "activity.more":return loadActivity(true);
      case "backend.install":return installBackend();
      case "backend.account.connect":return connectAccount();
      case "backend.account.cancel":app.account=await api("backend/account/login",{method:"DELETE"});clearTimeout(app.accountTimer);render();return;
      case "config.save":return saveConfig(false);
      case "config.reload":app.doc=await api("config");setNotice("Latest saved configuration loaded. Your unsaved field edits are preserved.");return;
      case "setup.finish":return saveConfig(true);
      case "draft.discard":await clearDraftNamespace("config");setNotice("Configuration draft discarded.");return;
      case "runtime.start":case "runtime.stop":await api("lifecycle",{method:"POST",body:{action:action.split(".")[1]}});app.doc=await api("config");await refresh(false);setNotice(action.endsWith("start")?"Agent enabled for ACP connections.":"Agent runtime stopped. Its management service remains available.");return;
      case "refresh":await refresh();setNotice("Agent observations refreshed.");return;
      case "profile.save":{const key=payload.profile;if(!Object.hasOwn(profileNames,key))throw new Error("Unknown profile.");await flushDrafts();const text=String(draft(`profiles.${key}`,app.profiles[key]||""));await api(`profiles/${key}`,{method:"PUT",body:{text}});app.profiles[key]=text;await discardProfileDraft(key);await loadProfiles();setNotice(`${profileNames[key]} profile saved.`);return;}
      case "profile.discard":await discardProfileDraft(payload.profile);setNotice(`${profileNames[payload.profile]} edits discarded.`);return;
      case "profile.package.open":app.profilePackageImport=null;app.profilePackageImportError="";await loadProfiles();app.profilePackageExpected=app.profilePackage?.revision;return command({type:"open_modal",modal:"profile-package"});
      case "profile.package.prepare":return prepareProfilePackage(payload.package,payload.expected_revision);
      case "profile.package.import":return applyProfilePackage(payload);
      case "profile.package.restore":return applyProfilePackage(payload,true);
      case "profile.package.history":return command({type:"set_filter",key:"profile_package_history",value:state().filters?.profile_package_history!==true});
      case "section.focus":if(["section.backend","section.limits","section.service"].includes(payload.target)&&!state().filters?.advanced)await command({type:"set_filter",key:"advanced",value:true});await command({type:"focus",target:payload.target});{const el=findControl(payload.target);if(el){el.scrollIntoView({behavior:"smooth",block:"start"});}}return;
      case "session.select":return command({type:"select_session",session_id:payload.session});
      case "session.clear":app.sessionDetail=null;return command({type:"select_session",session_id:null});
      case "event.inspect":return command({type:"open_modal",modal:`event:${payload.event}`});
      case "view.inspect":return command({type:"open_modal",modal:"view"});
      case "modal.close":app.tokenOnce=null;return command({type:"close_modal"});
      case "secret.new":await command({type:"set_draft",path:"access.secret_name",value:""});return command({type:"open_modal",modal:"secret"});
      case "secret.edit":await command({type:"set_draft",path:"access.secret_name",value:payload.name});return command({type:"open_modal",modal:"secret"});
      case "secret.save":{const name=payload.name??draft("access.secret_name","");const value=payload.value??root.querySelector("[data-secret-input]")?.value;if(!/^[A-Za-z0-9_-]{1,80}$/.test(name))throw new Error("Use 1–80 letters, numbers, hyphens, or underscores for the secret name.");if(!value)throw new Error("Enter a secret value.");await api(`secrets/${encodeURIComponent(name)}`,{method:"PUT",body:{value}});const input=root.querySelector("[data-secret-input]");if(input)input.value="";await command({type:"close_modal"});app.secrets=asArray(await api("secrets"));setNotice(`Secret “${name}” stored. Its value will not be shown again.`);return;}
      case "secret.remove":await api(`secrets/${encodeURIComponent(payload.name)}`,{method:"DELETE"});app.secrets=asArray(await api("secrets"));setNotice(`Secret “${payload.name}” removed.`);return;
      case "token.new":return command({type:"open_modal",modal:"token"});
      case "token.create":{await flushDrafts();const name=payload.name??draft("access.token_name","");const scopes=payload.scopes??["observe","ui","acp","context:owner","tools:use","profiles","work:read","work","work:execute","memory:read","memory","admin"].filter(s=>draft(`access.token_scopes.${s}`,s==="observe"));const hours=Number(payload.hours??draft("access.token_hours",24));if(!name.trim())throw new Error("Give the token a recognizable name.");if(!scopes.length)throw new Error("Choose at least one token permission.");if(!Number.isFinite(hours)||hours<=0)throw new Error("Set a positive token lifetime.");const result=await api("tokens",{method:"POST",body:{name,scopes,expires_in_seconds:Math.round(hours*3600)}});app.tokenOnce=result.token||result.value||result.access_token;app.tokens=asArray(await api("tokens"));await command({type:"open_modal",modal:"token-created"});render();return {created:true,id:result.id};}
      case "token.revoke":await api(`tokens/${encodeURIComponent(payload.token)}`,{method:"DELETE"});app.tokens=asArray(await api("tokens"));setNotice("Access token revoked.");return;
      case "connection.copy":await navigator.clipboard.writeText(attachInfo());setNotice("ACP connection information copied.");return;
      case "token.copy":if(!app.tokenOnce)throw new Error("The token is no longer available in this browser.");await navigator.clipboard.writeText(app.tokenOnce);announce("Token copied.");return;
      case "notice.dismiss":app.notice="";render();return;
      case "auth.logout":await api("auth/logout",{method:"POST",body:{}});signedOut();return;
      default:throw new Error(`Unknown action: ${action}`);
    }
  }

  async function refresh(showErrors=true) {
    if(!app.session)return;
    const requests=["status","connections","sessions",`events?after=${app.latestEvent}`,"backend","context/providers","profile-use","identity"];
    const results=await Promise.allSettled(requests.map(p=>api(p)));
    if(results[7]?.status==="fulfilled")app.identity=results[7].value;
    if(results[0].status==="fulfilled"){app.status=results[0].value;app.online=true;if(app.status.config_revision!==undefined&&app.status.config_revision!==app.doc.revision&&!hasDrafts()){try{app.doc=await api("config");}catch(e){if(showErrors)app.error=e.message;}}}else {app.online=false;if(showErrors)app.error=results[0].reason.message;}
    if(results[1].status==="fulfilled")app.connections=asArray(results[1].value);
    if(results[2].status==="fulfilled")app.sessions=asArray(results[2].value);
    if(results[3].status==="fulfilled")mergeEvents(results[3].value?.events||asArray(results[3].value));
    if(results[4].status==="fulfilled")app.backend=results[4].value;
    if(results[5].status==="fulfilled"){app.contextProviders=asArray(results[5].value.providers);app.contextError="";}else{app.contextError="Conversation sources could not be loaded.";}
    if(results[6]?.status==="fulfilled"){app.profileUse=results[6].value;app.profileUseError="";}else{app.profileUse=null;app.profileUseError="Profile delivery evidence could not be loaded.";}
    if(currentRoute()==="connections")await loadDevelopment();
    if(currentRoute()==="memory")await loadMemory();
    if(["work","organization"].includes(currentRoute()))await loadWork();
    await Promise.all([loadRecentActivity(),currentRoute()==="activity"?loadActivity():Promise.resolve(),currentRoute()==="profiles"?loadProfiles():Promise.resolve()]);
    render();
  }
  async function loadRecentActivity() {
    try{const data=await api("activity?limit=5&diagnostics=false&category=all");app.activityRecent=asArray(data);app.recentActivityError="";}catch(e){app.recentActivityError=e.message;}
  }
  async function loadActivity(append=false) {
    const request=++app.activityRequest,filters=state().filters||{};
    const query=new URLSearchParams({limit:"100",diagnostics:String(filters.diagnostics===true),category:activityCategory(filters),search:String(filters.search||"")});
    if(append&&app.activityNext!=null)query.set("before",app.activityNext);
    app.activityLoading=true;app.activityError="";if(!append&&app.activityQuery!==query.toString())app.activity=[];app.activityQuery=query.toString();render();
    try{const data=await api(`activity?${query}`);if(request!==app.activityRequest)return;app.activity=append?app.activity.concat(asArray(data)):asArray(data);app.activityNext=data.next_before;app.activityHasMore=data.has_more===true;}catch(e){if(request===app.activityRequest)app.activityError=e.message;}finally{if(request===app.activityRequest){app.activityLoading=false;render();}}
  }
  async function installBackend() {
    app.backendBusy=true;render();
    try{await api("backend/install",{method:"POST",body:{}});app.doc=await api("config");app.backend=await api("backend");await migrateSetupDraft();setNotice("Runtime installed. Check its account connection to finish setup.");}finally{app.backendBusy=false;render();}
  }
  async function connectAccount() {
    app.backendBusy=true;render();
    try{app.account=await api("backend/account/login",{method:"POST",body:{}});if(app.account.state==="connected")app.backend=await api("backend");scheduleAccountPoll();}finally{app.backendBusy=false;render();}
  }
  function scheduleAccountPoll() {
    clearTimeout(app.accountTimer);
    if(app.account?.state!=="pending")return;
    app.accountTimer=setTimeout(async()=>{
      if(!app.session)return;
      try{app.account=await api("backend/account/login");if(app.account.state==="connected"){app.backend=await api("backend");setNotice("ChatGPT account connected to this agent.");}else{render();scheduleAccountPoll();}}catch(e){app.account={state:"failed",message:e.message};render();}
    },2000);
  }
  function mergeEvents(events) {
    const bySeq=new Map(app.events.map(e=>[e.seq,e]));
    for(const event of events){if(!event||event.seq===undefined)continue;bySeq.set(event.seq,event);app.latestEvent=Math.max(app.latestEvent,Number(event.seq)||0);}
    app.events=[...bySeq.values()].sort((a,b)=>Number(a.seq)-Number(b.seq)).slice(-1000);
  }
  function streamEvent(event) {
    let data;try{data=JSON.parse(event.data);}catch{return;}
    const item=data.event || data;
    if(item.kind==="view.changed" || event.type==="view.changed"){
      const change=item.data?.view || item.data || item.view || item;
      if(change.id===app.view?.id && Number(change.revision)>Number(app.view.revision)){
        api(`views/${encodeURIComponent(change.id)}`).then(view=>applyView(view,true)).catch(fail);
      }
    }
    if(item.seq!==undefined)mergeEvents([item]);
    app.online=true;
    if(!String(item.kind||event.type).startsWith("view.")){clearTimeout(app.refreshTimer);app.refreshTimer=setTimeout(()=>refresh(false),650);}
  }
  function startEvents() {
    app.source?.close();app.source=new EventSource(new URL(`events/stream?after=${app.latestEvent}`,API),{withCredentials:true});
    app.source.onmessage=streamEvent;app.source.addEventListener("view.changed",streamEvent);
    app.source.onopen=()=>{app.online=true;app.streamOnline=true;};app.source.onerror=()=>{app.streamOnline=false;};
    clearInterval(app.poll);app.poll=setInterval(()=>refresh(false),15000);
  }
  function signedOut() {
    app.source?.close();clearInterval(app.poll);clearTimeout(app.accountTimer);clearTimeout(app.nativeAccountTimer);app.nativeAccount=null;app.nativeModels=null;app.account=null;app.backend=null;app.source=null;app.streamOnline=false;app.session=null;app.csrf="";app.tokenOnce=null;app.profilePackage=null;app.profilePackageImport=null;app.profilePackageImportError="";app.profilePackageExpected=null;app.view=null;app.draftPending.clear();app.loading=false;render();
  }
  async function migrateSetupDraft() {
    if(app.doc.config.setup_complete||app.backend?.kind!=="codex"||!app.backend.installed)return;
    const draft=state().draft||{},values={...(draft.config?.backend||{})};
    for(const [key,value] of Object.entries(draft))if(key.startsWith("config.backend."))values[key.slice(15)]=value;
    const entries=Object.entries(values);if(!entries.length)return;
    const neutral=([key,value])=>{
      if(key==="command")return value==null||typeof value==="string"&&!value.trim();
      if(key==="working_directory")return value==null||["","${AGENT_HOME}/workspace","${AGENT_HOME}/work"].includes(value);
      if(!["args","allowed_working_roots","env","secret_env"].includes(key))return false;
      if(typeof value==="string"){try{value=JSON.parse(value);}catch{return false;}}
      return value!=null&&typeof value==="object"&&Object.keys(value).length===0;
    };
    if(!entries.every(neutral))return;
    await patchView(next=>{if(next.draft?.config)delete next.draft.config.backend;for(const key of Object.keys(next.draft||{}))if(key.startsWith("config.backend."))delete next.draft[key];});
    app.notice="Your installed runtime is selected. Your agent’s name, purpose, and other edits have been kept.";
  }
  function storageKey(){return `agent-instance-view:${base.pathname}:${app.doc.config.instance_id}`;}
  async function enterSession(session) {
    app.session=session;app.csrf=session.csrf;app.error="";app.notice="";
    const [doc,status,backend]=await Promise.all([api("config"),api("status"),api("backend")]);app.doc=doc;app.status=status;app.backend=backend;
    if(backend.kind==="codex"&&!backend.auth_ready){try{app.account=await api("backend/account/login");scheduleAccountPoll();}catch{app.account=null;}}
    const data=await Promise.allSettled([api("schema"),api("connections"),api("sessions"),api("events?after=0"),api("secrets"),api("tokens"),...Object.keys(profileNames).map(k=>api(`profiles/${k}`))]);
    if(data[0].status==="fulfilled")app.schema=data[0].value;
    if(data[1].status==="fulfilled")app.connections=asArray(data[1].value);
    if(data[2].status==="fulfilled")app.sessions=asArray(data[2].value);
    if(data[3].status==="fulfilled")mergeEvents(data[3].value?.events||[]);
    if(data[4].status==="fulfilled")app.secrets=asArray(data[4].value);
    if(data[5].status==="fulfilled")app.tokens=asArray(data[5].value);
    Object.keys(profileNames).forEach((k,i)=>{if(data[6+i].status==="fulfilled")app.profiles[k]=data[6+i].value?.text||"";});
    try{const sources=await api("context/providers");app.contextProviders=asArray(sources.providers);app.contextError="";}catch{app.contextError="Conversation sources could not be loaded.";}
    let viewId;try{viewId=sessionStorage.getItem(storageKey());}catch{}
    if(viewId){try{app.view=await api(`views/${encodeURIComponent(viewId)}`);}catch{app.view=null;}}
    if(!app.view){app.view=await api("views",{method:"POST",body:{label:`Browser · ${navigator.platform||"web"}`}});try{sessionStorage.setItem(storageKey(),app.view.id);}catch{}if(doc.config.setup_complete)await command({type:"navigate",route:"dashboard"});}
    await migrateSetupDraft();
    if(backend.kind==="native")await loadNativeAccount(false);
    try{app.profileUse=await api("profile-use");}catch{}
    if(currentRoute()==="memory")await loadMemory();
    if(["work","organization"].includes(currentRoute()))await loadWork();
    if(state().route==="setup"&&Number(state().step)>1)await command({type:"navigate",route:"setup",step:1});
    await Promise.all([loadRecentActivity(),currentRoute()==="activity"?loadActivity():Promise.resolve(),loadProfiles()]);
    app.loading=false;app.lastAck=0;render();startEvents();if(state().selected_session)await loadSessionDetail();
  }

  root.addEventListener("submit",async event=>{
    if(event.target.id!=="login-form")return;event.preventDefault();const form=event.target;const username=form.elements.username.value,password=form.elements.password.value;
    app.busy=true;app.error="";form.querySelector("button").disabled=true;
    try{const session=await api("auth/login",{method:"POST",body:{username,password}});form.elements.password.value="";await enterSession(session);}catch(e){app.error=e.message;app.loading=false;}finally{app.busy=false;render();}
  });
  root.addEventListener("click",async event=>{
    const node=event.target.closest("[data-action]");if(!node || node.disabled)return;
    const action=node.dataset.action;if(["edit-draft","filter","auth.login"].includes(action))return;
    event.preventDefault();
    try{await perform(action,{...node.dataset});}catch(e){fail(e);}
  });
  root.addEventListener("input",event=>{
    const node=event.target;if(node.hasAttribute("data-sensitive"))return;
    if(node.dataset.draft)queueDraft(node.dataset.draft,node.type==="checkbox"?node.checked:node.value);
    if(node.dataset.filter&&node.type!=="checkbox"&&node.tagName!=="SELECT"){clearTimeout(app.timers.get(`filter:${node.dataset.filter}`));app.timers.set(`filter:${node.dataset.filter}`,setTimeout(()=>command({type:"set_filter",key:node.dataset.filter,value:node.value}).catch(fail),250));}
  });
  root.addEventListener("change",event=>{
    const node=event.target;if(node.hasAttribute("data-sensitive"))return;
    if(node.dataset.control==="identity.avatar.file"){
      const file=node.files?.[0];if(!file)return;
      if(file.size>5*1024*1024){fail(new Error("Choose an image smaller than 5 MB."));return;}
      const reader=new FileReader();reader.onerror=()=>fail(new Error("This image could not be read."));reader.onload=()=>savePortrait({media_type:file.type,data:String(reader.result).split(",")[1]}).catch(fail);reader.readAsDataURL(file);return;
    }
    if(node.dataset.control==="profile.package.file"){
      const file=node.files?.[0];if(!file)return;
      if(file.size>1024*1024){app.profilePackageImport=null;app.profilePackageImportError="Choose a profile package smaller than 1 MB.";render();return;}
      file.text().then(value=>prepareProfilePackage(value,app.profilePackageExpected)).catch(e=>{app.profilePackageImportError=e.message;render();});return;
    }
    if(node.dataset.draft && node.type==="checkbox")queueDraft(node.dataset.draft,node.checked);
    if(node.dataset.filter&&(node.tagName==="SELECT"||node.type==="checkbox")){
      const value=node.type==="checkbox"?node.checked:node.value;
      command({type:"set_filter",key:node.dataset.filter,value}).then(()=>node.dataset.filter==="diagnostics"&&!value&&state().filters?.category==="diagnostics"?command({type:"set_filter",key:"category",value:"all"}):null).catch(fail);
    }
  });
  let applyingFocus=false;
  root.addEventListener("focusin",event=>{
    const target=event.target.dataset.control;if(!target || !app.view || applyingFocus)return;
    if(state().focus===target)return;
    clearTimeout(app.timers.get("focus"));app.timers.set("focus",setTimeout(()=>{applyingFocus=true;command({type:"focus",target}).catch(()=>{}).finally(()=>{applyingFocus=false;});},180));
  });
  window.addEventListener("scroll",()=>{
    if(!app.view)return;clearTimeout(app.timers.get("scroll"));
    app.timers.set("scroll",setTimeout(()=>{const x=Math.round(window.scrollX),y=Math.round(window.scrollY);if(Math.abs((state().scroll?.y||0)-y)>3||Math.abs((state().scroll?.x||0)-x)>3)command({type:"scroll",x,y}).catch(()=>{});},250));
  },{passive:true});
  document.addEventListener("keydown",event=>{
    if(event.key==="Escape" && state().modal){perform("modal.close").catch(fail);}
    if(event.key==="Tab" && state().modal){const els=[...root.querySelectorAll('.modal button:not(:disabled),.modal input,.modal textarea,.modal select')];const first=els[0],last=els.at(-1);if(event.shiftKey&&document.activeElement===first){event.preventDefault();last?.focus();}else if(!event.shiftKey&&document.activeElement===last){event.preventDefault();first?.focus();}}
  });

  window.agentInstanceControl=Object.freeze({
    version:"1",
    getState:()=>app.view?clone(app.view):null,
    getDisplay:()=>({id:app.view?.id||null,visible:document.visibilityState==="visible",base:base.href}),
    actions:()=>[...root.querySelectorAll("[data-action]")].map(el=>({action:el.dataset.action,control:el.dataset.control||null,label:el.getAttribute("aria-label")||el.textContent.trim().slice(0,100),enabled:!el.disabled,...Object.fromEntries(Object.entries(el.dataset).filter(([key])=>!['action','control'].includes(key)))})).concat([{action:"identity.avatar.upload",control:"identity.avatar.file",label:"Upload agent image",enabled:!!app.session,parameters:["media_type","data","expected_revision"]}]),
    command:value=>command(value),
    perform:(action,payload)=>perform(action,payload),
    setDraft:(path,value)=>command({type:"set_draft",path,value})
  });
  window.addEventListener("message",async event=>{
    if(!app.session || event.source!==window.parent || event.source===window)return;
    if(!(app.doc?.config.server.embed_origins||[]).includes(event.origin))return;
    const request=event.data;if(!request || request.protocol!=="agent-instance.ui/1" || typeof request.id!=="string")return;
    try{let result;if(request.type==="get_state")result=window.agentInstanceControl.getState();else if(request.type==="command")result=await command(request.command);else if(request.type==="actions")result=window.agentInstanceControl.actions();else return;event.source.postMessage({protocol:"agent-instance.ui/1",id:request.id,ok:true,result},event.origin);}catch(e){event.source.postMessage({protocol:"agent-instance.ui/1",id:request.id,ok:false,error:e.message},event.origin);}
  });
  (async()=>{try{const session=await api("auth/session");await enterSession(session);}catch(e){app.session=null;app.loading=false;if(e.status!==401)app.error=e.message;render();}})();
})();
