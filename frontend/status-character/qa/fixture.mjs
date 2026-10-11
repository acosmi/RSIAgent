import {createStatusCharacter} from '../character.mjs';
import {createV3Scene} from '../v3-scene.mjs';
const $=s=>document.querySelector(s),report=$('#report'),restore=$('#restore');
const job=(state='running',extra={})=>({schema_version:'rsia.management_job.v1',id:'mock-visual-job',operation:'evaluation.start',state,generation:1,cancel_requested:false,...extra});
const validation=()=>job('succeeded',{operation:'evaluation.status',result:{result:'evaluation_status',ticket_id:'mock-ticket',lifecycle:'running',planned_targets:1,has_formal_terminal:false}});
const character=createStatusCharacter($('#character'),{onPresentationChange:settings=>{restore.hidden=!settings.hidden;}});
let token=character.selectSource('mock-visual','mock-visual-job');character.showUnavailable('disconnected');
restore.addEventListener('click',()=>character.setPresentation({hidden:false}));
$('#background').addEventListener('change',event=>{$('#backdrop').className='qa-backdrop qa-'+event.target.value;});
$('#state').addEventListener('change',event=>{
  const state=event.target.value;
  if(state==='disconnected'||state==='stale')character.showUnavailable(state==='stale'?'stale':'disconnected');
  else character.updateSnapshot(state==='validate'?validation():state==='cancel'?job('running',{cancel_requested:true}):job(state),token,'mock-visual');
});
let clicks=0;$('#behind').addEventListener('click',()=>{$('#behind').textContent=`点击穿透核验：${++clicks}`;});
$('#record-view').addEventListener('click',()=>{$('#interaction-report').textContent=JSON.stringify({recorded_at:new Date().toISOString(),component:character.getDiagnostics(),clicks},null,2);});
const fixedCanvas=$('#fixed-frame');let fixedScene=null;
const frame={width:320,height:340,dpr:1,pose:'validate',label:'验证中',theme:'sky',effect:'sweep',reducedMotion:false,artifacts:true,snap:true};
function fixedRender(scene,time=.6,patch={}){scene.renderFrame({...frame,time:0,...patch});scene.renderFrame({...frame,time,...patch});return scene.capturePixels();}
try{fixedScene=createV3Scene(fixedCanvas);fixedRender(fixedScene);}catch{report.textContent=JSON.stringify({webgl2:false,acceptance:'blocked'},null,2);}
// Native canvas PNG preserves alpha for private visual evidence. This export
// belongs only to the explicit mock fixture and adds no production endpoint.
const alphaExport=document.createElement('img');alphaExport.id='alpha-export';alphaExport.hidden=true;alphaExport.alt='';document.body.append(alphaExport);
document.fonts.ready.then(()=>{if(fixedScene){fixedScene.invalidateFont();fixedRender(fixedScene);alphaExport.src=fixedCanvas.toDataURL('image/png');}});
const nextFrame=()=>new Promise(resolve=>requestAnimationFrame(resolve));
const pause=ms=>new Promise(resolve=>setTimeout(resolve,ms));
const check=(condition,message)=>{if(!condition)throw Error(message);};
const sha=async pixels=>Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256',pixels))).map(v=>v.toString(16).padStart(2,'0')).join('');
function alphaMetrics({width,height,pixels}){
  let clear=0,partial=0,opaque=0,minX=width,minY=height,maxX=-1,maxY=-1;
  for(let y=0;y<height;y++)for(let x=0;x<width;x++){const a=pixels[(y*width+x)*4+3];if(a===0)clear++;else{if(a===255)opaque++;else partial++;minX=Math.min(minX,x);minY=Math.min(minY,y);maxX=Math.max(maxX,x);maxY=Math.max(maxY,y);}}
  const corners=[[0,0],[width-1,0],[0,height-1],[width-1,height-1]].map(([x,y])=>pixels[(y*width+x)*4+3]);return {width,height,clear,partial,opaque,corners,bounds:[minX,minY,maxX,maxY]};
}
async function run(){
  const output={kind:'real-browser-webgl2-with-explicit-mock-facts',started_at:new Date().toISOString(),user_agent:navigator.userAgent,device_pixel_ratio:devicePixelRatio,system_reduced_motion:matchMedia('(prefers-reduced-motion: reduce)').matches,font_stack:'PingFang SC, Microsoft YaHei, Noto Sans CJK SC, sans-serif',checks:[],limits:['No live backend connection','No original HTML rehosting; original-vs-component RGBA comparison not executed','No claims about backend acceptance, adoption or improvement']};
  const step=async(name,fn)=>{try{const result=await fn();output.checks.push({name,passed:true,result});}catch(error){output.checks.push({name,passed:false,error:error.message});}report.textContent=JSON.stringify(output,null,2);};
  $('#run-checks').disabled=true;
  await document.fonts.ready;fixedScene?.invalidateFont();
  await step('actual_webgl2_alpha_and_fixed_frame',async()=>{
    check(fixedScene,'WebGL2 scene unavailable');const gl=fixedCanvas.getContext('webgl2'),capture=fixedRender(fixedScene),metrics=alphaMetrics(capture);
    check(metrics.corners.every(a=>a===0),'Canvas corner is not transparent');check(metrics.clear>20000&&metrics.partial>100&&metrics.opaque>1000,'Unexpected alpha coverage');
    return {...metrics,sha256:await sha(capture.pixels),context_attributes:gl.getContextAttributes(),version:gl.getParameter(gl.VERSION),renderer:gl.getParameter(gl.RENDERER),gl_error:gl.getError(),scene:fixedScene.getDiagnostics()};
  });
  await step('deterministic_frames_and_independent_palette',async()=>{
    check(fixedScene,'WebGL2 unavailable');const first=fixedRender(fixedScene),repeat=fixedRender(fixedScene),otherCanvas=document.createElement('canvas'),other=createV3Scene(otherCanvas);
    try{
      const same=fixedRender(other);let mismatches=0;for(let i=0;i<first.pixels.length;i++)if(first.pixels[i]!==repeat.pixels[i]||first.pixels[i]!==same.pixels[i])mismatches++;
      check(mismatches===0,'Equal fixed frames differ');const iris=fixedRender(other,.6,{theme:'iris'});check(await sha(iris.pixels)!==await sha(first.pixels),'Palette did not change');check(await sha(fixedRender(fixedScene).pixels)===await sha(first.pixels),'Second instance changed first instance');
      return {mismatched_channels:mismatches,sky_sha256:await sha(first.pixels),iris_sha256:await sha(iris.pixels)};
    }finally{other.dispose({releaseContext:true});}
  });
  await step('original_sweep_and_unchanged_text_texture',async()=>{
    check(fixedScene,'WebGL2 unavailable');const hashes=[];fixedScene.renderFrame({...frame,time:0});const revision=fixedScene.getDiagnostics().textureRevision;
    for(const time of [.3,.7,1.45,4.8]){fixedScene.renderFrame({...frame,time});hashes.push({time,sha256:await sha(fixedScene.capturePixels().pixels)});}
    check(new Set(hashes.map(h=>h.sha256)).size>1,'Animation frames are identical');check(fixedScene.getDiagnostics().textureRevision===revision,'Text texture uploaded per frame');fixedRender(fixedScene);return {revision,frames:hashes};
  });
  await step('real_component_reduced_motion_and_terminal_stop',async()=>{
    character.setPresentation({hidden:false,collapsed:false,reducedMotion:false,paused:false});character.updateSnapshot(validation(),token,'mock-visual');await pause(160);
    const working=character.getDiagnostics();if(!working.systemReducedMotion)check(working.raf===1&&working.animated,'Work fact did not animate');
    character.setPresentation({reducedMotion:true});const reduced=character.getDiagnostics();await pause(130);check(character.getDiagnostics().renders===reduced.renders&&character.getDiagnostics().raf===0,'Reduced motion still loops');
    character.setPresentation({reducedMotion:false});character.updateSnapshot(job('succeeded'),token,'mock-visual');const stopped=character.getDiagnostics();await pause(130);check(stopped.view.label==='已结束'&&!stopped.view.animated&&character.getDiagnostics().raf===0&&character.getDiagnostics().renders===stopped.renders,'Terminal still loops');
    character.showUnavailable('stale');check(character.getDiagnostics().view.label==='不可用'&&character.getDiagnostics().raf===0,'Stale source still animates');return {working,reduced,terminal:stopped,stale:character.getDiagnostics()};
  });
  await step('scope_invalidation_and_equal_generation_terminal',async()=>{
    const old=token;token=character.selectSource('mock-visual','mock-visual-job');character.updateSnapshot(job(),token,'mock-visual');character.updateSnapshot(job('succeeded'),token,'mock-visual');check(character.getDiagnostics().view.label==='已结束','Equal generation terminal rejected');character.updateSnapshot(validation(),old,'mock-visual');check(character.getDiagnostics().view.label==='已结束','Old scope published');return {label:character.getDiagnostics().view.label};
  });
  await step('unchanged_facts_keep_one_animation_chain',async()=>{
    character.updateSnapshot(validation(),token,'mock-visual');const before=character.getDiagnostics();character.updateSnapshot(validation(),token,'mock-visual');const after=character.getDiagnostics();check(before.loops===after.loops&&before.scene.textureRevision===after.scene.textureRevision,'Identical read restarted animation or uploaded text');character.showUnavailable('disconnected');return {before_loops:before.loops,after_loops:after.loops,texture_revision:after.scene.textureRevision};
  });
  await step('hidden_resume_requires_a_fresh_fact',async()=>{
    character.updateSnapshot(validation(),token,'mock-visual');character.setPresentation({hidden:true});const hidden=character.getDiagnostics();check(!hidden.visible&&hidden.raf===0,'Hidden component animates');character.setPresentation({hidden:false});const resumed=character.getDiagnostics();check(resumed.view.label==='同步中'&&resumed.raf===0,'Resume replayed an old working fact');character.updateSnapshot(job('succeeded'),token,'mock-visual');check(character.getDiagnostics().view.label==='已结束','Fresh fact failed to publish');return {hidden,resumed,fresh_label:character.getDiagnostics().view.label};
  });
  await step('native_frame_sizes_dpr_cap_and_narrow_camera',async()=>{
    const canvas=document.createElement('canvas'),scene=createV3Scene(canvas),frames=[];
    try{
      for(const [width,height,dpr,focus]of [[320,340,1,false],[320,340,1.25,false],[320,340,1.5,false],[320,340,2,false],[160,340,1,false],[320,340,1,true]]){
        const capture=fixedRender(scene,.6,{width,height,dpr,focus}),metrics=alphaMetrics(capture);check(capture.width===Math.round(width*Math.min(dpr,1.65))&&capture.height===Math.round(height*Math.min(dpr,1.65)),'DPR size cap mismatch');check(metrics.opaque>100,'Frame is empty');frames.push({css_size:[width,height],requested_dpr:dpr,focus,...metrics,sha256:await sha(capture.pixels)});
      }
      return {frames,credit:'Actual WebGL frame sizes; requested DPR values are fixtures, not OS or browser zoom emulation'};
    }finally{scene.dispose({releaseContext:true});}
  });
  await step('collapse_releases_full_scene_and_expand_recreates',async()=>{
    character.setPresentation({collapsed:true});const collapsed=character.getDiagnostics();check(collapsed.scene===null&&collapsed.raf===0,'Collapsed scene retained');check(collapsed.releaseProof?.checked>0&&collapsed.releaseProof.alive===0,'GPU resources retained');check($('#character .sc-badge img')?.src.startsWith('data:image/png'),'Collapsed head did not use the actual V3 rendering');character.setPresentation({collapsed:false});check(character.getDiagnostics().scene?.geometryCount>0,'Expanded scene unavailable');return {collapsed,expanded:character.getDiagnostics()};
  });
  await step('small_host_uses_original_head_and_releases_scene',async()=>{
    const host=document.createElement('div');host.className='qa-small-host';document.body.append(host);const instance=createStatusCharacter(host);
    try{
      await nextFrame();await nextFrame();const small=instance.getDiagnostics();check(small.spaceLimited&&small.scene===null&&small.raf===0,'Narrow host retained or clipped the full scene');check(host.querySelector('.sc-badge img')?.src.startsWith('data:image/png'),'Narrow host lost the actual rendered head');
      host.classList.add('qa-roomy');await nextFrame();await nextFrame();const roomy=instance.getDiagnostics();check(!roomy.spaceLimited&&roomy.scene?.geometryCount>0&&roomy.view.label==='未连接','Roomy host did not restore a neutral unconnected scene');return {small,roomy};
    }finally{instance.dispose();host.remove();}
  });
  await step('100_real_component_create_render_dispose_cycles',async()=>{
    let checked=0,minGeometry=Infinity,maxAlive=0;const host=$('#recycle-host');
    for(let i=0;i<100;i++){
      const instance=createStatusCharacter(host),before=instance.getDiagnostics();check(before.scene?.geometryCount>0,`Cycle ${i} did not render`);minGeometry=Math.min(minGeometry,before.scene.geometryCount);instance.dispose();instance.dispose();const after=instance.getDiagnostics();check(after.disposed&&after.raf===0&&after.releaseProof?.checked>0,`Cycle ${i} did not dispose`);checked+=after.releaseProof.checked;maxAlive=Math.max(maxAlive,after.releaseProof.alive);check(after.releaseProof.alive===0,`Cycle ${i} retained GPU handles`);
      if(i%5===4){report.textContent=JSON.stringify({...output,recycle_progress:i+1},null,2);await nextFrame();}
    }
    check(host.children.length===0,'Disposed roots retained');return {cycles:100,checked_gpu_handles:checked,min_geometry_count:minGeometry,max_alive_handles:maxAlive,remaining_dom_roots:host.children.length};
  });
  await step('real_context_loss_fallback_and_restore',async()=>{
    const canvas=$('#character canvas'),gl=canvas.getContext('webgl2'),extension=gl.getExtension('WEBGL_lose_context');check(extension,'Context loss extension unavailable');
    const lost=new Promise(resolve=>canvas.addEventListener('webglcontextlost',resolve,{once:true}));extension.loseContext();await Promise.race([lost,pause(1200)]);check(character.getDiagnostics().scene===null&&character.getDiagnostics().raf===0,'Lost context still active');check(!$('#character .sc-fallback').hidden,'Fallback not visible');
    // Restoring inside the loss-event microtask is forbidden by the extension:
    // wait for that event dispatch to finish before requesting restoration.
    await nextFrame();
    const restored=new Promise(resolve=>canvas.addEventListener('webglcontextrestored',resolve,{once:true}));extension.restoreContext();await Promise.race([restored,pause(2000)]);await nextFrame();check(character.getDiagnostics().scene?.geometryCount>0,'Restored GPU scene unavailable: '+JSON.stringify(character.getDiagnostics()));return character.getDiagnostics();
  });
  output.completed_at=new Date().toISOString();output.passed=output.checks.every(v=>v.passed);report.textContent=JSON.stringify(output,null,2);$('#run-checks').disabled=false;
}
$('#run-checks').addEventListener('click',()=>run());
window.addEventListener('pagehide',()=>{character.dispose();fixedScene?.dispose({releaseContext:true});},{once:true});
