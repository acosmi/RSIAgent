import {createV3Scene} from './v3-scene.mjs';
import {createScopeGate} from './view-model.mjs';

const neutral=reason=>Object.freeze({label:reason==='syncing'?'同步中':reason==='disconnected'||reason==='inactive'?'未连接':'不可用',pose:'observe',animated:false,artifacts:false,reason});
const badge=`<svg viewBox="0 0 80 70" aria-hidden="true"><defs><linearGradient id="sc-shell" x2=".8" y2="1"><stop stop-color="#f5faff"/><stop offset="1" stop-color="#c6d7e6"/></linearGradient></defs><rect x="6" y="24" width="9" height="23" rx="4" fill="#77cfff"/><rect x="65" y="24" width="9" height="23" rx="4" fill="#77cfff"/><rect x="12" y="11" width="56" height="47" rx="18" fill="url(#sc-shell)"/><rect x="18" y="22" width="44" height="26" rx="9" fill="#04111d"/><path d="M24 30v11m32-11v11" stroke="#16ceff" stroke-width="2.5" stroke-linecap="round"/><rect x="31" y="9" width="18" height="3" rx="1.5" fill="#77cfff"/><path d="M34 56h12v8H34z" fill="#c6d7e6"/></svg>`;
const palettes=new Set(['sky','glacier','iris','silver']);

export function createStatusCharacter(container,options={}) {
  if(!container?.ownerDocument)throw TypeError('A DOM container is required');
  const doc=container.ownerDocument,win=doc.defaultView,gate=createScopeGate();
  const root=doc.createElement('section');root.className='sc-character';root.setAttribute('aria-label','RSIAgent 状态角色');
  root.innerHTML=`<div class="sc-stage"><canvas class="sc-canvas" aria-hidden="true"></canvas><div class="sc-fallback" hidden>${badge}</div></div><button type="button" class="sc-badge" aria-label="展开状态角色" hidden>${badge}</button><div class="sc-actions"><button type="button" class="sc-menu-button" aria-label="状态角色设置" aria-expanded="false"><span class="sc-status-dot" aria-hidden="true"></span><span class="sc-status" role="status" aria-live="polite" aria-atomic="true">未连接</span><svg class="sc-menu-icon" viewBox="0 0 16 14" aria-hidden="true"><circle cx="3" cy="7" r="1.2"/><circle cx="8" cy="7" r="1.2"/><circle cx="13" cy="7" r="1.2"/></svg></button></div><div class="sc-panel" hidden><button type="button" data-action="view">查看形象</button><button type="button" data-action="focus" aria-pressed="false">面屏特写</button><button type="button" data-action="pause" aria-pressed="false">暂停动态</button><label><input type="checkbox" data-option="reduced">减少动态</label><label><input type="checkbox" data-option="orbit" checked>流体回环</label><label>配色<select aria-label="角色配色"><option value="sky">天蓝</option><option value="glacier">冰青</option><option value="iris">鸢尾</option><option value="silver">银白</option></select></label><button type="button" data-action="collapse">收起角色</button><button type="button" data-action="hide">隐藏角色</button><p class="sc-detail">未连接现有任务</p></div>`;
  container.append(root);
  const $=s=>root.querySelector(s),canvas=$('canvas'),stage=$('.sc-stage'),fallback=$('.sc-fallback'),status=$('.sc-status'),panel=$('.sc-panel'),menu=$('.sc-menu-button'),detail=$('.sc-detail');
  const settings={paused:false,reducedMotion:false,orbit:true,theme:'sky',focus:false,collapsed:false,hidden:false,viewing:false};
  let disposed=false,scene=null,ownedContext=null,spaceLimited=false,hasSelection=false,raf=null,width=320,height=340,hostActive=true,intersecting=true,contextLost=false,failed=false,lastSceneError=null,contextRestores=0,view=neutral('disconnected'),yaw=0,targetYaw=0,pointerStartX=0,pointerStartYaw=0,elapsed=0,lastRender=null,firstFrame=true,pointer=null,renders=0,loops=0,activity=null,releaseProof=null;
  const media=win.matchMedia('(prefers-reduced-motion: reduce)'),listeners=[];
  const listen=(el,event,fn,opts)=>{el.addEventListener(event,fn,opts);listeners.push(()=>el.removeEventListener(event,fn,opts));};
  const visible=()=>!disposed&&hostActive&&intersecting&&!doc.hidden&&!settings.hidden;
  const collapsed=()=>settings.collapsed||spaceLimited;
  const moves=()=>view.animated&&!settings.paused&&!settings.reducedMotion&&!media.matches;
  const turning=()=>settings.viewing&&!settings.paused&&!settings.reducedMotion&&!media.matches&&Math.abs(targetYaw-yaw)>.0005;
  const running=()=>visible()&&!collapsed()&&!failed&&!contextLost&&Boolean(scene)&&(moves()||turning());
  function stop(){if(raf!==null)win.cancelAnimationFrame(raf);raf=null;lastRender=null;}
  function release(releaseContext=false){if(scene){scene.dispose({releaseContext});releaseProof=scene.getDiagnostics().releaseProof??null;scene=null;}if(releaseContext&&ownedContext&&!ownedContext.isContextLost())ownedContext.getExtension('WEBGL_lose_context')?.loseContext();firstFrame=true;}
  function showFallback(){fallback.hidden=false;canvas.hidden=true;}
  function captureBadge(){
    if(!scene||contextLost||failed)return;
    try{
      scene.renderFrame({pose:'observe',label:'未连接',time:0,snap:true,width:112,height:112,dpr:1,theme:settings.theme,orbit:false,focus:true,reducedMotion:true,effect:'steady',artifacts:false});
      const image=doc.createElement('img');image.alt='';image.setAttribute('aria-hidden','true');image.src=canvas.toDataURL('image/png');$('.sc-badge').replaceChildren(image);
    }catch{/* Keep the static fallback badge if a snapshot cannot be read. */}
  }
  function ensureScene(){
    if(scene||failed||contextLost||collapsed()||!visible())return;
    try{scene=createV3Scene(canvas);ownedContext=canvas.getContext('webgl2');firstFrame=true;fallback.hidden=true;canvas.hidden=false;lastSceneError=null;}catch(error){ownedContext=canvas.getContext('webgl2',{alpha:true,antialias:true,premultipliedAlpha:true,powerPreference:'low-power'});lastSceneError=String(error?.message??'Scene creation failed');failed=true;release();showFallback();}
  }
  function draw(delta=0,snap=false){
    if(!scene||!visible()||collapsed())return;
    try{
      scene.renderFrame({pose:view.pose,label:view.label,time:elapsed,delta,snap:firstFrame||snap,width,height,dpr:win.devicePixelRatio||1,theme:settings.theme,orbit:settings.orbit,focus:settings.focus,yaw,reducedMotion:!moves(),paused:settings.paused,effect:moves()?'sweep':'steady',artifacts:view.artifacts});
      firstFrame=false;renders++;
    }catch(error){lastSceneError=String(error?.message??'Scene rendering failed');failed=true;stop();release();showFallback();}
  }
  function frame(timestamp){
    raf=null;if(!running())return;
    if(lastRender===null||timestamp-lastRender>=1000/30){
      const delta=lastRender===null?1/30:Math.min((timestamp-lastRender)/1000,.1);
      if(moves())elapsed+=delta;
      yaw+=(targetYaw-yaw)*(1-Math.exp(-delta*9));if(Math.abs(targetYaw-yaw)<=.0005)yaw=targetYaw;
      lastRender=timestamp;draw(delta);
    }
    if(running())raf=win.requestAnimationFrame(frame);
  }
  function updateTurn(){
    if(settings.paused||settings.reducedMotion||media.matches){yaw=targetYaw;reconcile();return;}
    if(running()&&raf===null){loops++;lastRender=null;raf=win.requestAnimationFrame(frame);}
  }
  function reconcile(){
    if(disposed)return;
    if(settings.paused||settings.reducedMotion||media.matches||!settings.viewing)yaw=targetYaw;
    const nextActivity=visible()&&!collapsed();
    if(nextActivity!==activity){
      if(activity!==null){view=neutral(nextActivity?(hasSelection?'syncing':'disconnected'):'inactive');status.textContent=view.label;$('.sc-badge').setAttribute('aria-label',`展开状态角色，${view.label}`);detail.textContent=nextActivity?(hasSelection?'正在读取当前任务':'未连接现有任务'):'当前角色未显示';}
      activity=nextActivity;options.onActiveChange?.(activity);
    }
    stop();root.hidden=settings.hidden;root.classList.toggle('sc-collapsed',collapsed());root.classList.toggle('sc-viewing',settings.viewing);root.classList.toggle('sc-status-active',moves()&&nextActivity);
    $('.sc-badge').hidden=!collapsed();$('.sc-badge').disabled=spaceLimited;stage.hidden=collapsed();
    if(spaceLimited)$('.sc-badge').setAttribute('aria-label','显示区域较窄，状态角色已收起');
    if(collapsed()){captureBadge();release();panel.hidden=true;menu.setAttribute('aria-expanded','false');return;}
    ensureScene();if(failed||contextLost)showFallback();else draw(0,!moves());
    if(running()){loops++;raf=win.requestAnimationFrame(frame);}
  }
  function publish(next){
    if(disposed||!next)return next;
    const unchanged=['label','pose','animated','artifacts','reason'].every(key=>view[key]===next[key]);
    view=next;
    if(!next.animated)yaw=targetYaw;
    if(status.textContent!==next.label)status.textContent=next.label;
    $('.sc-badge').setAttribute('aria-label',`展开状态角色，${next.label}`);
    const reasons={syncing:'正在读取当前任务',disconnected:'未连接现有任务',inactive:'当前角色未显示',authorization:'当前任务授权不可用',missing:'当前任务不可用',stale:'当前来源已失效',timeout:'读取超时',connection:'连接暂不可用'};
    detail.textContent=next.reason?reasons[next.reason]??'当前任务快照不可用':`${next.label} · 由当前任务状态更新`;
    if(!unchanged)reconcile();return next;
  }
  function presentation(patch){
    if(disposed)return;
    for(const key of ['paused','reducedMotion','orbit','focus','collapsed','hidden','viewing'])if(Object.hasOwn(patch,key))settings[key]=Boolean(patch[key]);
    if(Object.hasOwn(patch,'theme')){if(!palettes.has(patch.theme))throw TypeError('Unknown palette');settings.theme=patch.theme;}
    if(!settings.viewing&&pointer!==null){if(stage.hasPointerCapture?.(pointer))stage.releasePointerCapture(pointer);pointer=null;}
    stage.tabIndex=settings.viewing?0:-1;stage.setAttribute('aria-label','拖动旋转角色，左右箭头调整，Home 复位');
    $('[data-action="view"]').textContent=settings.viewing?'结束查看':'查看形象';
    $('[data-action="focus"]').setAttribute('aria-pressed',String(settings.focus));
    $('[data-action="pause"]').setAttribute('aria-pressed',String(settings.paused));
    $('[data-option="reduced"]').checked=settings.reducedMotion;
    $('[data-option="orbit"]').checked=settings.orbit;$('select').value=settings.theme;
    reconcile();options.onPresentationChange?.(Object.freeze({...settings}));
  }
  listen(menu,'click',()=>{panel.hidden=!panel.hidden;menu.setAttribute('aria-expanded',String(!panel.hidden));});
  listen($('.sc-badge'),'click',()=>presentation({collapsed:false}));
  listen(panel,'click',event=>{const action=event.target.closest('button')?.dataset.action;if(!action)return;if(action==='view')presentation({viewing:!settings.viewing});else if(action==='focus')presentation({focus:!settings.focus});else if(action==='pause')presentation({paused:!settings.paused});else if(action==='collapse')presentation({collapsed:true,viewing:false});else if(action==='hide')presentation({hidden:true,viewing:false});});
  listen($('[data-option="reduced"]'),'change',event=>presentation({reducedMotion:event.target.checked}));
  listen($('[data-option="orbit"]'),'change',event=>presentation({orbit:event.target.checked}));
  listen($('select'),'change',event=>presentation({theme:event.target.value}));
  listen(root,'keydown',event=>{if(event.key==='Escape'){panel.hidden=true;menu.setAttribute('aria-expanded','false');presentation({viewing:false});menu.focus();}});
  listen(stage,'pointerdown',event=>{if(!settings.viewing||event.button!==0)return;pointer=event.pointerId;pointerStartX=event.clientX;pointerStartYaw=targetYaw;stage.setPointerCapture(pointer);});
  listen(stage,'pointermove',event=>{if(pointer!==event.pointerId)return;targetYaw=Math.max(-1.9,Math.min(1.9,pointerStartYaw+(event.clientX-pointerStartX)*.006));updateTurn();});
  const endPointer=event=>{if(pointer===event.pointerId){if(stage.hasPointerCapture(pointer))stage.releasePointerCapture(pointer);pointer=null;}};
  listen(stage,'pointerup',endPointer);listen(stage,'pointercancel',endPointer);
  listen(stage,'keydown',event=>{if(!settings.viewing)return;if(event.key==='ArrowLeft')targetYaw-=.15;else if(event.key==='ArrowRight')targetYaw+=.15;else if(event.key==='Home'){targetYaw=yaw=0;event.preventDefault();reconcile();return;}else return;event.preventDefault();targetYaw=Math.max(-1.9,Math.min(1.9,targetYaw));updateTurn();});
  listen(doc,'visibilitychange',()=>{if(doc.hidden)publish(neutral('inactive'));else reconcile();});
  listen(media,'change',()=>reconcile());
  listen(canvas,'webglcontextlost',event=>{event.preventDefault();contextLost=true;stop();release();showFallback();});
  listen(canvas,'webglcontextrestored',()=>{if(disposed)return;contextRestores++;contextLost=false;failed=false;reconcile();});
  const resize=new win.ResizeObserver(entries=>{
    const available=entries.find(entry=>entry.target===container)?.contentRect;
    if(available)spaceLimited=available.width<240;
    const box=entries.find(entry=>entry.target===stage)?.contentRect;
    if(box&&box.width>0&&box.height>0){width=box.width;height=box.height;}
    reconcile();
  });resize.observe(stage);resize.observe(container);
  const intersection=new win.IntersectionObserver(entries=>{intersecting=entries[0]?.isIntersecting??true;reconcile();});intersection.observe(root);
  listen(win,'resize',()=>reconcile());
  doc.fonts?.ready.then(()=>{if(!disposed){scene?.invalidateFont();reconcile();}});
  presentation(options);
  return Object.freeze({
    selectSource(connection,id){hasSelection=typeof connection==='string'&&Boolean(connection)&&typeof id==='string'&&Boolean(id);const token=gate.select(connection,id);publish(neutral(hasSelection?'syncing':'disconnected'));return token;},
    updateSnapshot(job,token,connection){return publish(gate.accept(token,connection,job));},
    showUnavailable(reason='connection'){return publish(neutral(reason));},
    setActive(value){hostActive=Boolean(value);if(!hostActive)publish(neutral('inactive'));else reconcile();},
    setPresentation:presentation,
    getDiagnostics(){return Object.freeze({disposed,visible:visible(),animated:running(),raf:raf===null?0:1,renders,loops,view:{...view},size:[width,height],yaw,targetYaw,spaceLimited,presentation:{...settings},systemReducedMotion:media.matches,contextLost,contextRestores,failed,lastSceneError,scene:scene?.getDiagnostics()??null,releaseProof});},
    dispose(){if(disposed)return;disposed=true;stop();gate.dispose();resize.disconnect();intersection.disconnect();for(const remove of listeners)remove();release(true);root.remove();options.onActiveChange?.(false);}
  });
}
