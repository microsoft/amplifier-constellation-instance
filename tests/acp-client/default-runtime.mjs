// Opt-in live model check. This sends one small prompt; it permits no client-side tools.
import assert from 'node:assert/strict';
import {spawn} from 'node:child_process';
import {Readable,Writable} from 'node:stream';
import * as acp from '@agentclientprotocol/sdk';
const [cwd,binary,home]=process.argv.slice(2);
const child=spawn(binary,['acp','--home',home],{stdio:['pipe','pipe','ignore']});
const timer=setTimeout(()=>child.kill('SIGKILL'),90000);
let text='',toolRequests=0;
try {
 const result=await acp.client({name:'default-runtime-validation'})
   .onRequest(acp.methods.client.session.requestPermission,()=>{toolRequests++;return {outcome:{outcome:'cancelled'}};})
   .onNotification(acp.methods.client.session.update,({params})=>{if(params.update.sessionUpdate==='agent_message_chunk'&&params.update.content?.type==='text')text+=params.update.content.text;})
   .connectWith(acp.ndJsonStream(Writable.toWeb(child.stdin),Readable.toWeb(child.stdout)),async ctx=>{
     const init=await ctx.request(acp.methods.agent.initialize,{protocolVersion:acp.PROTOCOL_VERSION,clientInfo:{name:'default-runtime-validation',version:'1'},clientCapabilities:{}});
     assert.equal(init.protocolVersion,1);
     const session=await ctx.request(acp.methods.agent.session.new,{cwd,mcpServers:[]});
     const reply=await ctx.request(acp.methods.agent.session.prompt,{sessionId:session.sessionId,prompt:[{type:'text',text:'This is a connection test. Reply with only the word READY. Do not use tools, inspect files, or do any other work.'}]});
     assert.equal(reply.stopReason,'end_turn');assert.equal(text.trim(),'READY');assert.equal(toolRequests,0);
     return {protocol:init.protocolVersion,stop_reason:reply.stopReason,expected_response:true,tool_requests:toolRequests};
   });
 console.log(JSON.stringify({success:true,...result}));
} finally {clearTimeout(timer);child.kill();}
