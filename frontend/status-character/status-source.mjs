// A single, non-overlapping read chain. This module never issues business writes.
export function createStatusSource({read,onSnapshot,onState=()=>{},onSelect=()=>{},clock=globalThis,activeInterval=1500,idleInterval=5000,timeout=4000}) {
  if(typeof read!=='function'||typeof onSnapshot!=='function')throw TypeError('Read and snapshot callbacks are required');
  let disposed=false,active=true,selection=null,epoch=0,timer=null,inflight=null,deadline=null,reads=0;
  const clearTimer=()=>{if(timer!==null)clock.clearTimeout(timer);timer=null;};
  function invalidate(){++epoch;clearTimer();if(deadline!==null)clock.clearTimeout(deadline);deadline=null;inflight?.controller.abort();}
  function schedule(delay){clearTimer();if(!disposed&&active&&selection&&!inflight)timer=clock.setTimeout(()=>{timer=null;poll();},delay);}
  async function poll(){
    if(disposed||!active||!selection||inflight)return;
    const scope=selection,token=epoch,controller=new AbortController(),request={controller,token};
    inflight=request;reads++;let delay=idleInterval;
    deadline=clock.setTimeout(()=>controller.abort('timeout'),timeout);
    try{
      const job=await read({signal:controller.signal,selection:scope});
      if(disposed||!active||token!==epoch||scope!==selection)return;
      if(controller.signal.aborted){onState('unavailable','timeout');return;}
      const result=onSnapshot(job,scope);
      if(result?.animated)delay=activeInterval;
    }catch(error){
      if(!disposed&&active&&token===epoch&&scope===selection){
        const code=controller.signal.aborted?'timeout':error?.status===401||error?.status===403?'authorization':error?.status===404?'missing':error?.status===409?'stale':'connection';
        onState('unavailable',code);
      }
    }finally{
      if(inflight===request){
        if(deadline!==null)clock.clearTimeout(deadline);deadline=null;inflight=null;
        // Old reads cannot publish; a replacement scope starts only after the
        // cancelled request settles, keeping the one-request invariant.
        if(!disposed&&active&&selection)schedule(token===epoch?delay:0);
      }
    }
  }
  return Object.freeze({
    select(connection,id){
      if(disposed)throw Error('Source is disposed');
      invalidate();selection=typeof connection==='string'&&connection&&typeof id==='string'&&id?Object.freeze({connection,id}):null;
      if(selection){onSelect(selection);onState(active?'syncing':'inactive');schedule(0);}else onState('disconnected');
    },
    setActive(value){if(disposed||active===Boolean(value))return;active=Boolean(value);invalidate();if(active&&selection){onState('syncing');schedule(0);}else if(selection)onState('inactive');},
    refresh(){if(!disposed&&active&&selection)schedule(0);},
    getDiagnostics(){return Object.freeze({disposed,active,inflight:inflight?1:0,timer:timer!==null,reads,selected:Boolean(selection)});},
    dispose(){if(disposed)return;disposed=true;invalidate();selection=null;}
  });
}

export async function readLocalStatus({signal}) {
  const response=await fetch('/status',{signal,cache:'no-store',credentials:'same-origin',headers:{Accept:'application/json'}});
  if(!response.ok){const error=Error('Status read unavailable');error.status=response.status;throw error;}
  return response.json();
}
