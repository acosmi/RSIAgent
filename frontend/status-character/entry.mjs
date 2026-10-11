import {createStatusCharacter} from './character.mjs';
import {createStatusSource,readLocalStatus} from './status-source.mjs';
const show=document.querySelector('#show-character');
let mounted=null;
function mount(){
  const abort=new AbortController();let source=null,selectionToken=null,disposed=false,active=false;
  const character=createStatusCharacter(document.querySelector('#character'),{
    onActiveChange:value=>{active=value;source?.setActive(value);},
    onPresentationChange:settings=>{show.hidden=!settings.hidden;}
  });
  const reveal=()=>character.setPresentation({hidden:false});show.addEventListener('click',reveal);
  async function connect(){
    try{
      const response=await fetch('/connection',{signal:abort.signal,cache:'no-store',credentials:'same-origin'});
      if(!response.ok)throw Error('Connection unavailable');
      const config=await response.json();if(disposed)return;
      if(!config.configured||typeof config.job_id!=='string'||!config.job_id){character.showUnavailable('disconnected');return;}
      const connection='fixed-local-bridge';
      source=createStatusSource({read:readLocalStatus,
        onSelect:scope=>{selectionToken=character.selectSource(scope.connection,scope.id);},
        onSnapshot:job=>character.updateSnapshot(job,selectionToken,connection),
        onState:(state,reason)=>character.showUnavailable(state==='syncing'?'syncing':state==='inactive'?'inactive':reason??'disconnected')
      });
      source.setActive(active);source.select(connection,config.job_id);
    }catch{if(!disposed)character.showUnavailable('connection');}
  }
  connect();
  return ()=>{if(disposed)return;disposed=true;abort.abort();source?.dispose();character.dispose();show.removeEventListener('click',reveal);show.hidden=true;};
}
mounted=mount();
window.addEventListener('pagehide',()=>{mounted?.();mounted=null;});
window.addEventListener('pageshow',event=>{if(event.persisted&&!mounted)mounted=mount();});
