import test from 'node:test';
import assert from 'node:assert/strict';
import {mapManagementSnapshot,createScopeGate} from '../view-model.mjs';
import {createStatusSource} from '../status-source.mjs';
import {createBridge,projectJob} from '../bridge.mjs';

const job=(state='running',extra={})=>({schema_version:'rsia.management_job.v1',id:'mock-job',operation:'evaluation.start',state,generation:1,cancel_requested:false,...extra});
const tick=()=>new Promise(resolve=>setImmediate(resolve));
function fakeClock(){let time=0,id=0;const pending=new Map();return {setTimeout(fn,delay){pending.set(++id,{fn,at:time+delay});return id;},clearTimeout(key){pending.delete(key);},async advance(ms){time+=ms;for(const [key,value]of [...pending])if(value.at<=time){pending.delete(key);value.fn();await tick();}},get pending(){return pending.size;}};}
test('management/ticket success never grants acceptance or adoption',()=>{
  assert.equal(mapManagementSnapshot(job('succeeded'),'mock-job').label,'已结束');
  for(const [lifecycle,label,animated]of [['reserved','待执行',false],['running','验证中',true],['ready_for_final','待收口',false],['stop_requested','停止中',false],['draining','收尾中',false],['early_stopped','已早停',false],['complete_batch','已结束',false],['invalid','异常',false]]){
    const view=mapManagementSnapshot(job('succeeded',{operation:'evaluation.status',result:{result:'evaluation_status',ticket_id:'mock-ticket',lifecycle,planned_targets:1,has_formal_terminal:true}}),'mock-job');
    assert.equal(view.label,label);assert.equal(view.animated,animated);
  }
});
test('unknown, wrong scope, cancellation and failure stop animation',()=>{
  for(const snapshot of [null,job('unknown'),job('running',{generation:-1}),job('running',{schema_version:'legacy'}),job('running',{id:'another'})])assert.equal(mapManagementSnapshot(snapshot,'mock-job').animated,false);
  for(const state of ['draining','succeeded','failed','cancelled','blocked'])assert.equal(mapManagementSnapshot(job(state),'mock-job').animated,false);
  assert.equal(mapManagementSnapshot(job('running',{cancel_requested:true}),'mock-job').label,'取消中');
});
test('same-generation terminal is accepted; revoked callbacks cannot publish',()=>{
  const gate=createScopeGate(),old=gate.select('a','mock-job');assert.equal(gate.accept(old,'a',job()).animated,true);
  assert.equal(gate.accept(old,'a',job('succeeded')).label,'已结束');
  assert.equal(gate.accept(old,'a',job('running',{generation:0})),null);
  const next=gate.select('b','mock-job');assert.equal(gate.accept(old,'a',job()),null);assert.equal(gate.accept(next,'b',job()).animated,true);
  assert.equal(gate.accept(next,'b',job('running',{id:'wrong'})).label,'不可用');gate.dispose();assert.equal(gate.accept(next,'b',job()),null);
});
test('transport has one request; selection/hidden/disposal revoke late results',async()=>{
  const clock=fakeClock(),requests=[],published=[],states=[];
  const source=createStatusSource({clock,read:({signal,selection})=>new Promise(resolve=>requests.push({signal,selection,resolve})),onSnapshot:(value,scope)=>{published.push([value,scope]);return {animated:true};},onState:(...args)=>states.push(args)});
  source.select('a','mock-job');await clock.advance(0);assert.equal(requests.length,1);
  source.refresh();await clock.advance(0);assert.equal(requests.length,1);
  source.select('b','next-job');assert.equal(requests[0].signal.aborted,true);await clock.advance(0);assert.equal(requests.length,1);
  requests[0].resolve(job());await tick();assert.equal(published.length,0);await clock.advance(0);assert.equal(requests.length,2);
  source.setActive(false);requests[1].resolve(job('running',{id:'next-job'}));await tick();await clock.advance(10000);assert.equal(published.length,0);assert.equal(requests.length,2);
  source.setActive(true);await clock.advance(0);requests[2].resolve(job('running',{id:'next-job'}));await tick();assert.equal(published.length,1);
  await clock.advance(1499);assert.equal(requests.length,3);await clock.advance(1);assert.equal(requests.length,4);
  source.dispose();requests[3].resolve(job());await tick();await clock.advance(10000);assert.equal(published.length,1);assert.equal(clock.pending,0);assert.equal(source.getDiagnostics().disposed,true);
});
test('timeout and auth errors fail neutral, then use idle backoff',async()=>{
  const clock=fakeClock(),states=[];let request;
  const source=createStatusSource({clock,read:({signal})=>new Promise((resolve,reject)=>{request={resolve,reject,signal};signal.addEventListener('abort',()=>reject(Error('aborted')),{once:true});}),onSnapshot:()=>({animated:true}),onState:(...v)=>states.push(v)});
  source.select('a','mock-job');await clock.advance(0);await clock.advance(4000);assert.deepEqual(states.at(-1),['unavailable','timeout']);assert.equal(request.signal.aborted,true);
  await clock.advance(5000);const denied=Error('denied');denied.status=403;request.reject(denied);await tick();assert.deepEqual(states.at(-1),['unavailable','authorization']);source.dispose();
});
test('a late connection selected while inactive stays paused until resumed',async()=>{
  const clock=fakeClock(),states=[];let reads=0;
  const source=createStatusSource({clock,read:async()=>{reads++;return job();},onSnapshot:()=>({animated:true}),onState:state=>states.push(state)});
  source.setActive(false);source.select('mock-connection','mock-job');
  assert.equal(states.at(-1),'inactive');await clock.advance(10000);assert.equal(reads,0);assert.equal(source.getDiagnostics().timer,false);
  source.setActive(true);assert.equal(states.at(-1),'syncing');await clock.advance(0);assert.equal(reads,1);source.dispose();
});
test('bridge projection excludes credential/lease/input/owner/full results',()=>{
  const projected=projectJob(job('succeeded',{operation:'evaluation.status',owner_actor:'private',lease_token:'private',private_input_ref:'private',result:{result:'evaluation_status',ticket_id:'mock-ticket',lifecycle:'running',planned_targets:2,has_formal_terminal:false,raw_trace:'private'}}));
  assert.deepEqual(Object.keys(projected).sort(),['cancel_requested','generation','id','operation','result','schema_version','state']);assert.equal(JSON.stringify(projected).includes('private'),false);
});
test('preview serves explicit static whitelist and has no backend connection',async t=>{
  let reads=0;const bridge=await createBridge({qa:true,fetchBackend:()=>{reads++;throw Error('Must not connect');}});t.after(()=>bridge.close());
  const get=(route,options)=>fetch(bridge.origin+route,options);
  assert.equal((await get('/')).status,200);assert.equal((await get('/qa/')).status,200);assert.deepEqual(await(await get('/connection')).json(),{configured:false,job_id:null});assert.equal((await get('/status')).status,503);
  for(const route of ['/bridge.mjs','/tests/contracts.test.mjs','/.git/config','/README.md','/v3-scene.mjs?path=private','/%2e%2e/private'])assert.equal((await get(route)).status,404);
  assert.equal((await get('/status',{method:'POST'})).status,405);assert.equal((await get('/',{headers:{Origin:'https://foreign.invalid'}})).status,403);assert.equal(reads,0);
});
test('fixed backend route is GET only; mock transport cannot redirect or choose scopes',async t=>{
  const requests=[];const bridge=await createBridge({connection:{url:'http://127.0.0.1:7788',jobId:'mock-job',token:'mock-test-token'},fetchBackend:async(url,options)=>{requests.push({url,options});return new Response(JSON.stringify(job('succeeded',{lease_token:'hidden'})),{status:200});}});t.after(()=>bridge.close());
  const result=await(await fetch(bridge.origin+'/status')).json();assert.equal(result.state,'succeeded');assert.equal(Object.hasOwn(result,'lease_token'),false);assert.equal(requests[0].url,'http://127.0.0.1:7788/v1/manage/jobs/mock-job');assert.equal(requests[0].options.method,'GET');assert.equal(requests[0].options.redirect,'error');
  assert.equal((await fetch(bridge.origin+'/status?job=another')).status,404);assert.equal(requests.length,1);
});
test('closing a status response aborts its pending backend read promptly',async()=>{
  let signal,release,started;
  const pending=new Promise(resolve=>{started=resolve;});
  const bridge=await createBridge({connection:{url:'http://127.0.0.1:7788',jobId:'mock-job',token:'mock-test-token'},fetchBackend:(_url,options)=>new Promise((resolve,reject)=>{
    signal=options.signal;release=()=>reject(Error('Test cleanup'));started();
    signal.addEventListener('abort',()=>reject(Error('Aborted mock backend')),{once:true});
  })});
  const controller=new AbortController(),request=fetch(bridge.origin+'/status',{signal:controller.signal}).catch(error=>error);
  try{await pending;controller.abort();await request;await new Promise(resolve=>setTimeout(resolve,100));assert.equal(signal.aborted,true);}finally{release();await bridge.close();}
});
