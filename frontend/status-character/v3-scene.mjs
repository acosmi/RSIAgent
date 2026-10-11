/** Local isolated prototype: derived from the verified V3 transparent design.
 * Private source provenance is recorded in ../input-pins.json and ../visual-source-lock.json.
 * No backend connection, business state loop, global API, or production acceptance.
 */
export function createV3Scene(canvas) {
let renderer=null,robot=null,scene=null,disposed=false,time=0,state='observe',reduced=true,orbitOn=true,visorOn=true,currentYaw=0,displayLabel='未连接';
const TAU=Math.PI*2, clamp=(x,a,b)=>Math.max(a,Math.min(b,x)), mix=(a,b,t)=>a+(b-a)*t;
const vadd=(a,b)=>a.map((v,i)=>v+b[i]), vsub=(a,b)=>a.map((v,i)=>v-b[i]), vmul=(a,s)=>a.map(v=>v*s), dot=(a,b)=>a.reduce((s,v,i)=>s+v*b[i],0);
const norm=a=>{const l=Math.hypot(...a)||1;return a.map(v=>v/l)}, cross=(a,b)=>[a[1]*b[2]-a[2]*b[1],a[2]*b[0]-a[0]*b[2],a[0]*b[1]-a[1]*b[0]];
const identity=()=>new Float32Array([1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1]);
function mul(a,b){const o=new Float32Array(16);for(let c=0;c<4;c++)for(let r=0;r<4;r++)o[c*4+r]=a[r]*b[c*4]+a[4+r]*b[c*4+1]+a[8+r]*b[c*4+2]+a[12+r]*b[c*4+3];return o}
function compose(p,r,s){let [x,y,z]=r;const cx=Math.cos(x),sx=Math.sin(x),cy=Math.cos(y),sy=Math.sin(y),cz=Math.cos(z),sz=Math.sin(z);return new Float32Array([cy*cz*s[0],(sx*sy*cz+cx*sz)*s[0],(-cx*sy*cz+sx*sz)*s[0],0,-cy*sz*s[1],(-sx*sy*sz+cx*cz)*s[1],(cx*sy*sz+sx*cz)*s[1],0,sy*s[2],-sx*cy*s[2],cx*cy*s[2],0,...p,1])}
function lookAt(eye,target){const z=norm(vsub(eye,target)),x=norm(cross([0,1,0],z)),y=cross(z,x);return new Float32Array([x[0],y[0],z[0],0,x[1],y[1],z[1],0,x[2],y[2],z[2],0,-dot(x,eye),-dot(y,eye),-dot(z,eye),1])}
function perspective(fov,aspect,near=.1,far=70){const f=1/Math.tan(fov/2),n=1/(near-far);return new Float32Array([f/aspect,0,0,0,0,f,0,0,0,0,(far+near)*n,-1,0,0,2*far*near*n,0])}
function transform(m,p){return [m[0]*p[0]+m[4]*p[1]+m[8]*p[2]+m[12],m[1]*p[0]+m[5]*p[1]+m[9]*p[2]+m[13],m[2]*p[0]+m[6]*p[1]+m[10]*p[2]+m[14]]}
const hex=h=>[parseInt(h.slice(1,3),16)/255,parseInt(h.slice(3,5),16)/255,parseInt(h.slice(5,7),16)/255];
class Node{constructor(geo=null,mat=null){this.geo=geo;this.mat=mat;this.p=[0,0,0];this.r=[0,0,0];this.s=[1,1,1];this.children=[];this.visible=true;this.world=identity()}add(n){this.children.push(n);return n}pos(x,y,z){this.p=[x,y,z];return this}rot(x,y,z){this.r=[x,y,z];return this}scale(x,y=x,z=x){this.s=[x,y,z];return this}}
const cache=new Map();
function geometry(key,make){if(!cache.has(key))cache.set(key,make());return cache.get(key)}
function geoFrom(P,N,UV,I,mode='tri'){return {positions:new Float32Array(P),normals:new Float32Array(N),uvs:new Float32Array(UV),indices:new Uint16Array(I),mode}}
function sphere(w=32,h=20){return geometry(`s${w},${h}`,()=>{const P=[],N=[],U=[],I=[];for(let y=0;y<=h;y++)for(let x=0;x<=w;x++){const a=x/w*TAU,b=y/h*Math.PI;const p=[Math.sin(b)*Math.cos(a),Math.cos(b),Math.sin(b)*Math.sin(a)];P.push(...p);N.push(...p);U.push(x/w,y/h)}for(let y=0;y<h;y++)for(let x=0;x<w;x++){const k=y*(w+1)+x;I.push(k,k+1,k+w+1,k+1,k+w+2,k+w+1)}return geoFrom(P,N,U,I)})}
function rounded(w,h,d,r,seg=12){return geometry(`b${w},${h},${d},${r},${seg}`,()=>{const P=[],N=[],U=[],I=[];const sz=[w/2,h/2,d/2],rr=sz.map(v=>Math.min(v,r)),inn=sz.map((v,i)=>Math.max(0,v-rr[i]));const faces=[[0,1,2,1],[0,2,1,-1],[1,2,0,1],[1,0,2,-1],[2,0,1,1],[2,1,0,-1]];for(const [a,b,c,sg] of faces){const base=P.length/3;for(let j=0;j<=seg;j++)for(let i=0;i<=seg;i++){const q=[0,0,0];q[a]=sz[a]*sg;q[b]=(i/seg*2-1)*sz[b];q[c]=(j/seg*2-1)*sz[c];const center=q.map((v,k)=>clamp(v,-inn[k],inn[k]));const nn=norm(q.map((v,k)=>(v-center[k])/rr[k]));const p=center.map((v,k)=>v+nn[k]*rr[k]);const n=norm(nn.map((v,k)=>v/rr[k]));P.push(...p);N.push(...n);U.push(i/seg,j/seg)}for(let j=0;j<seg;j++)for(let i=0;i<seg;i++){let k=base+j*(seg+1)+i;I.push(k,k+1,k+seg+1,k+1,k+seg+2,k+seg+1)}}return geoFrom(P,N,U,I)})}
function tube(path,r=.018,sides=7,closed=false){const pts=path,N=[],P=[],U=[],I=[];for(let i=0;i<pts.length;i++){const prev=pts[i===0?(closed?pts.length-2:0):i-1],next=pts[i===pts.length-1?(closed?1:i):i+1];let dir=norm(vsub(next,prev)),ref=Math.abs(dir[1])>.9?[1,0,0]:[0,1,0];const x=norm(cross(dir,ref)),y=cross(dir,x);for(let j=0;j<=sides;j++){const a=j/sides*TAU,n=vadd(vmul(x,Math.cos(a)),vmul(y,Math.sin(a)));P.push(...vadd(pts[i],vmul(n,r)));N.push(...n);U.push(i/(pts.length-1),j/sides)}}for(let i=0;i<pts.length-1;i++)for(let j=0;j<sides;j++){const k=i*(sides+1)+j;I.push(k,k+1,k+sides+1,k+1,k+sides+2,k+sides+1)}return geoFrom(P,N,U,I)}
function torus(radius,thick=.025,segments=64){return geometry(`t${radius},${thick},${segments}`,()=>tube(Array.from({length:segments+1},(_,i)=>[radius*Math.cos(i/segments*TAU),radius*Math.sin(i/segments*TAU),0]),thick,8,true))}
function rectPath(w,h,r,segments=12){const P=[];for(let c=0;c<4;c++){const a=c*Math.PI/2, cx=(c===0||c===3?1:-1)*(w/2-r),cy=(c<2?1:-1)*(h/2-r);for(let i=0;i<=segments;i++){const t=a+i/segments*Math.PI/2;P.push([cx+Math.cos(t)*r,cy+Math.sin(t)*r,0])}}P.push(P[0]);return P}
function outline(w,h,r,thick=.009){return geometry(`o${w},${h},${r},${thick}`,()=>tube(rectPath(w,h,r),thick,6,true))}
function plateEdge(w,d){const x=w/2,z=d/2;return geometry(`pe${w},${d}`,()=>tube([[-x,0,-z],[x,0,-z],[x,0,z],[-x,0,z],[-x,0,-z]],.008,5,true))}
function plane(){return geometry('plane',()=>geoFrom([-1,-1,0,1,-1,0,1,1,0,-1,1,0],[0,0,1,0,0,1,0,0,1,0,0,1],[0,0,1,0,1,1,0,1],[0,1,2,0,2,3]))}
function ribbon(){return geometry('ribbon',()=>{const P=[],N=[],U=[],I=[],steps=190;for(let i=0;i<=steps;i++){const t=i/steps,a=t*TAU*1.22;const p=[Math.cos(a)*1.8,-1.3+t*3.35,Math.sin(a)*.84],side=norm([.06*Math.cos(a),1,.05*Math.sin(a)]);for(let j=0;j<2;j++){const q=vadd(p,vmul(side,(j-.5)*(.15+.06*Math.sin(t*Math.PI))));P.push(...q);N.push(...norm([Math.cos(a),.2,Math.sin(a)]));U.push(t,j)}if(i<steps){const k=i*2;I.push(k,k+1,k+2,k+1,k+3,k+2)}}return geoFrom(P,N,U,I)})}
function material(color,rough=.4,metal=.0,emit=0,alpha=1,mode=0){return {color:hex(color),rough,metal,emit,alpha,mode}}
const mats={white:material('#edf6ff',.29,.10),pearl:material('#f5faff',.22,.04),inset:material('#b4d4f0',.26,.24),dark:material('#20394f',.26,.62),black:material('#04111d',.30,.10,0,1,2),silver:material('#93b2ca',.24,.65),accent:material('#77cfff',.27,.20),glass:material('#75d7f7',.14,.20,.06,.30),core:material('#31d6ff',.2,.12,.64,.65),light:material('#16ceff',.35,.0,1.2),bright:material('#baf9ff',.2,0,1.65),ribbon:material('#7edfff',.32,.18,.28,.19,3),line:material('#a7edff',.35,0,.7,.53),glow:material('#5eeaff',.5,0,1,.18,1),shadow:material('#5588af',1,0,0,.11,4)};
function mesh(parent,g,m,p=[0,0,0],s=[1,1,1],r=[0,0,0]){const n=parent.add(new Node(g,m));n.p=[...p];n.s=[...s];n.r=[...r];return n}
const ell=(p,m,x,y,z,a,b=a,c=a)=>mesh(p,sphere(),m,[x,y,z],[a,b,c]);
function buildHand(parent,side){const g=parent.add(new Node());mesh(g,rounded(.24,.25,.14,.065,8),mats.pearl,[0,-.09,.035]);mesh(g,rounded(.15,.10,.025,.025,6),mats.inset,[0,-.075,-.046]);const fingers=[];for(let i=0;i<3;i++){const f=g.add(new Node()).pos((i-1)*.082,-.19,.025);ell(f,mats.silver,0,-.035,0,.045);mesh(f,rounded(.066,.14,.077,.03,5),mats.white,[0,-.09,0]);const end=f.add(new Node()).pos(0,-.16,0);mesh(end,rounded(.063,.115,.074,.029,5),mats.pearl,[0,-.046,0]);end.r[0]=-.18;fingers.push(f)}const thumb=g.add(new Node()).pos(-side*.13,-.04,.045).rot(0,0,-side*.6);mesh(thumb,rounded(.08,.17,.09,.039,5),mats.white,[0,-.065,0]);ell(thumb,mats.pearl,0,-.15,.015,.046,.065,.045);fingers.push(thumb);return {g,fingers}}
function buildArm(root,side){
 const pivot=root.add(new Node()).pos(side*.665,.39,0);
 ell(pivot,mats.dark,0,0,0,.226);
 ell(pivot,mats.inset,side*.047,.012,.023,.232,.216,.216);
 ell(pivot,mats.pearl,side*.060,.012,.057,.234,.224,.244);
 mesh(pivot,torus(.176,.012,40),mats.light,[side*.21,0,0],[1,1,1],[0,Math.PI/2,0]);
 mesh(pivot,rounded(.325,.32,.335,.159,12),mats.pearl,[0,-.20,0]);
 const elbow=pivot.add(new Node()).pos(0,-.355,0);
 ell(elbow,mats.dark,0,0,0,.132);
 mesh(elbow,rounded(.354,.315,.35,.156,12),mats.white,[0,-.19,.01]);
 mesh(elbow,rounded(.18,.18,.025,.065,8),mats.inset,[0,-.185,-.157]);
 mesh(elbow,torus(.114,.011,32),mats.light,[0,-.332,0],[1,1,1],[Math.PI/2,0,0]);
 const wrist=elbow.add(new Node()).pos(0,-.376,0);
 ell(wrist,mats.dark,0,0,0,.083);
 const hand=buildHand(wrist,side);hand.g.s=[1.025,.92,1.06];
 return {pivot,elbow,wrist,...hand};
}
function buildLeg(root,side){
 const p=root.add(new Node()).pos(side*.274,-.425,.012);
 ell(p,mats.dark,0,-.022,0,.17);
 mesh(p,rounded(.38,.315,.387,.154,12),mats.white,[0,-.158,.02]);
 const k=p.add(new Node()).pos(0,-.30,0);
 ell(k,mats.dark,0,-.018,0,.13);
 ell(k,mats.accent,0,-.025,.091,.145,.10,.063);
 mesh(k,rounded(.405,.335,.41,.164,12),mats.pearl,[0,-.19,.039]);
 mesh(k,rounded(.17,.16,.035,.067,8),mats.inset,[0,-.19,-.158]);
 const foot=k.add(new Node()).pos(0,-.368,.055);
 mesh(foot,rounded(.409,.232,.478,.114,14),mats.white,[0,-.015,.089]);
 mesh(foot,rounded(.35,.054,.397,.026,8),mats.inset,[0,-.126,.084]);
 return {p,k,foot};
}
// Typography occupies a real 3D surface between the eyes, not a page overlay.
// 1024 x 480 monochrome mask is uploaded only when the label changes. Shader animates light.
const VISOR_FONT='"PingFang SC", "Microsoft YaHei", "Noto Sans CJK SC", sans-serif';
function makeVisor(head){
 const surface=document.createElement('canvas');surface.width=1024;surface.height=480;
 const ctx=surface.getContext('2d');if(!ctx)return null;
 const mat=material('#baf9ff',.5,0,1,1,5);mat.textureCanvas=surface;mat.textureVersion=0;
 const face=mesh(head,plane(),mat,[0,.006,.605],[.50,.234375,1]);
 return {face,mat,ctx,lastLabel:null,phaseStart:0,changedAt:-10,fontPx:260,fontWeight:800};
}
function updateVisor(current){
 if(!robot?.visor)return;
 const visor=robot.visor;visor.face.visible=visorOn;
 const label=displayLabel;
 if(visor.lastLabel===label)return;
 visor.lastLabel=label;visor.phaseStart=time;visor.changedAt=time;
 const ctx=visor.ctx;ctx.clearRect(0,0,1024,480);
 ctx.font=`800 260px ${VISOR_FONT}`;ctx.textBaseline='middle';ctx.textAlign='left';
 const chars=Array.from(label),spacing=14;
 let fontPx=260;
 let total=chars.reduce((n,c)=>n+ctx.measureText(c).width,0)+spacing*(chars.length-1);
 // Defensive fit for a longer future label; current five labels all have three characters.
 if(total>842){fontPx=Math.floor(fontPx*842/total);ctx.font=`800 ${fontPx}px ${VISOR_FONT}`;total=chars.reduce((n,c)=>n+ctx.measureText(c).width,0)+spacing*(chars.length-1);}
 visor.fontPx=fontPx;
 let x=(1024-total)/2;
 ctx.fillStyle='#fff';ctx.strokeStyle='#fff';ctx.lineWidth=2;ctx.lineJoin='round';
 // Tight baked halo; no blur on the readable core of the glyphs.
 ctx.shadowColor='rgba(170,236,255,.30)';ctx.shadowBlur=6;
 for(const c of chars){ctx.strokeText(c,x,218);ctx.fillText(c,x,218);x+=ctx.measureText(c).width+spacing;}
 ctx.shadowBlur=0;ctx.fillStyle='rgba(215,245,255,.70)';
 for(let i=0;i<3;i++){ctx.beginPath();ctx.arc(472+i*40,402,6.8,0,TAU);ctx.fill();}
 visor.mat.textureVersion++;
}
function buildRobot(){const root=new Node();const torso=root.add(new Node());ell(torso,mats.pearl,0,.038,0,.615,.547,.417);mesh(torso,rounded(.67,.26,.57,.129,14),mats.white,[0,-.357,0]);ell(torso,mats.inset,0,-.425,0,.375,.11,.28);for(const side of [-1,1]){mesh(torso,rounded(.13,.35,.17,.064,10),mats.inset,[side*.548,.005,.083],[1,1,1],[0,0,-side*.12]);mesh(torso,rounded(.025,.23,.035,.012,6),mats.light,[side*.552,.04,.167]);}
// Layered core: recessed dark glass, luminous trim, separated crystal plates.
const chest=torso.add(new Node()).pos(0,-.002,.05).scale(1,.87,1);
mesh(chest,rounded(.68,.82,.145,.21,14),mats.inset,[0,.07,.31]);mesh(chest,rounded(.62,.755,.12,.19,14),mats.dark,[0,.08,.38]);mesh(chest,outline(.615,.75,.18,.014),mats.light,[0,.08,.448]);mesh(chest,rounded(.55,.685,.042,.16,12),material('#10465b',.21,.26),[0,.08,.44]);const core=chest.add(new Node()).pos(0,.06,.475);const layers=[];for(let i=0;i<4;i++){const l=core.add(new Node()).pos(0,(i-1.5)*.128,0).rot(.38,.6,0);mesh(l,rounded(.32,.027,.28,.011,5),mats.core);mesh(l,plateEdge(.325,.285),mats.bright);layers.push(l)}mesh(chest,rounded(.49,.64,.025,.145,10),material('#82e9ff',.13,.1,.025,.08),[0,.08,.72]);mesh(chest,rounded(.15,.028,.021,.012,4),mats.bright,[0,-.36,.372]);
mesh(torso,rounded(.31,.15,.29,.07,10),mats.dark,[0,.606,0]);mesh(torso,rounded(.30,.035,.28,.016,6),mats.silver,[0,.668,0]);const head=torso.add(new Node()).pos(0,1.10,.028);mesh(head,rounded(1.64,1.10,.99,.391,24),mats.pearl);mesh(head,rounded(1.405,.832,.20,.265,22),mats.inset,[0,-.008,.423]);mesh(head,rounded(1.335,.777,.17,.244,24),mats.black,[0,-.008,.482]);mesh(head,outline(1.338,.781,.246,.008),material('#8acfea',.24,.4,.3),[0,-.008,.558]);const eyes=[];for(const side of [-1,1]){const e=head.add(new Node()).pos(side*.528,.028,.592).rot(0,0,-.08);mesh(e,rounded(.057,.214,.023,.028,12),mats.light);const halo=mesh(e,plane(),mats.glow,[0,0,.008],[.067,.152,1]);eyes.push(e)}
mesh(head,rounded(.67,.035,.46,.017,12),mats.accent,[0,.537,-.022],[1,1,1],[0,0,.02]);mesh(head,rounded(.30,.015,.32,.007,6),mats.inset,[-.11,.555,-.05]);for(const side of [-1,1]){ell(head,mats.dark,side*.811,-.01,-.018,.058,.261,.27);ell(head,mats.inset,side*.854,-.01,-.018,.06,.224,.23);ell(head,mats.accent,side*.902,-.01,-.018,.025,.173,.18);mesh(head,torus(.19,.018,48),mats.light,[side*.908,-.01,-.018],[1,1,1],[0,Math.PI/2,0]);ell(head,mats.dark,side*.928,-.01,-.018,.006,.135,.14);}
const arms=[buildArm(torso,-1),buildArm(torso,1)],legs=[buildLeg(torso,-1),buildLeg(torso,1)];const visor=makeVisor(head);return {root,torso,head,eyes,core,layers,arms,legs,visor}}
function buildScene(robot){const root=new Node();root.add(robot.root);const orbit=root.add(new Node()).rot(.10,0,-.22).pos(0,.055,-.22).scale(1,.84,1);mesh(orbit,ribbon(),mats.ribbon);for(let offset of [-.081,.081]){const path=Array.from({length:180},(_,i)=>{const t=i/179,a=t*TAU*1.22;return [Math.cos(a)*1.8,-1.3+t*3.35+offset,Math.sin(a)*.84]});mesh(orbit,tube(path,.009,5),mats.line)}const halo=root.add(new Node()).pos(0,.12,0).rot(.82,0,-.42);mesh(halo,torus(1.98,.012,120),material('#b3eaff',.5,0,.5,.3),[0,0,0],[1,1.08,1]);const particles=[];for(let i=0;i<19;i++){const r=.024+(i%5)*.008;const p=mesh(root,rounded(r*1.6,r*1.6,r*1.6,.008,3),i%3===0?mats.core:mats.inset);particles.push(p)}/* Transparent preview: no stage floor, floor rings or drop-shadow plane. */const cards=root.add(new Node());const dataCard=cards.add(new Node()).pos(-1.12,.42,.38).rot(0,.4,-.10);mesh(dataCard,rounded(.66,.42,.025,.047,8),material('#a6e9ff',.45,.1,.3,.22));mesh(dataCard,outline(.66,.42,.05,.006),mats.line,[0,0,.024]);for(let i=0;i<3;i++)mesh(dataCard,rounded(.34-i*.065,.022,.008,.008,3),mats.light,[-.06,.09-i*.085,.025]);const candidate=cards.add(new Node()).pos(.9,.0,.9);const cubes=[];for(let i=0;i<5;i++){const n=mesh(candidate,rounded(.18,.055,.18,.009,3),mats.core,[0,(i-2)*.12,0],[1,1,1],[.1,Math.PI/4,0]);mesh(n,plateEdge(.19,.19),mats.bright);cubes.push(n)}return {root,orbit,halo,particles,cards,dataCard,candidate,cubes}}
const VS=`#version 300 es
precision highp float;
in vec3 aPosition;in vec3 aNormal;in vec2 aUv;
uniform mat4 uModel;uniform mat4 uVP;
out vec3 vWorld;out vec3 vNormal;out vec3 vLocal;out vec2 vUv;
void main(){vec4 w=uModel*vec4(aPosition,1.);vWorld=w.xyz;vLocal=aPosition;vNormal=transpose(inverse(mat3(uModel)))*aNormal;vUv=aUv;gl_Position=uVP*w;}`;
const FS=`#version 300 es
precision highp float;
in vec3 vWorld;in vec3 vNormal;in vec3 vLocal;in vec2 vUv;
uniform vec3 uColor;uniform vec3 uCamera;uniform float uRough;uniform float uMetal;uniform float uEmission;uniform float uAlpha;uniform float uMode;uniform float uTime;uniform sampler2D uTexture;uniform vec3 uVisorFX;
out vec4 frag;
vec3 lin(vec3 c){return pow(c,vec3(2.2));}
void main(){
if(uMode>4.5){
 vec4 texel=texture(uTexture,vUv);if(texel.a<.012)discard;
 float mode=uVisorFX.x,t=uVisorFX.y,reveal=uVisorFX.z;
 float glint=0.;float strength=1.;
 if(mode>.5&&mode<1.5){
  // 1.45 s eased pass + 3.35 s quiet rest. No scrolling or revealing missing letters.
  float phase=mod(t,4.8);float p=clamp(phase/1.45,0.,1.);
  float center=mix(-.23,1.23,p*p*(3.-2.*p));
  float axis=vUv.x+(vUv.y-.5)*.16;
  glint=exp(-pow((axis-center)/.125,2.))*step(phase,1.45);
 }else if(mode>1.5&&mode<2.5){
  // Continuous, broad cyan/ice-white luster, confined to the letters.
  float wave=.5+.5*sin(vUv.x*6.283185-t*1.745329);
  glint=pow(wave,2.2)*.78;
 }else if(mode>2.5){
  // Never extinguish the text: brightness stays in the 84%-100% band.
  float b=.5+.5*sin(t*1.745329);
  strength=.84+.16*b;glint=.12*b;
 }
 vec3 base=uColor*vec3(.55,.89,.98);
 vec3 ice=mix(uColor,vec3(1.),.86);
 vec3 ink=mix(base,ice,clamp(glint+reveal*.24,0.,1.))*strength;
 // Three steady punctuation dots, not a false progress meter.
 if(vUv.y<.245)ink=uColor*.66;
 frag=vec4(texel.rgb*ink,texel.a*uAlpha);return;
}
float alpha=uAlpha;vec3 c=lin(uColor);vec3 n=normalize(vNormal);vec3 V=normalize(uCamera-vWorld);if(!gl_FrontFacing)n=-n;
if(uMode>.5&&uMode<1.5){vec2 q=vUv*2.-1.;float a=exp(-dot(q,q)*4.8);frag=vec4(uColor,alpha*a);return;}
if(uMode>3.5){vec2 q=vUv*2.-1.;alpha*=exp(-dot(q,q)*3.5);frag=vec4(uColor,alpha);return;}
vec3 L=normalize(vec3(-3.,5.5,5.)),L2=normalize(vec3(4.,1.5,3.)),L3=normalize(vec3(1.,4.,-5.));
float d=max(dot(n,L),0.),d2=max(dot(n,L2),0.),d3=max(dot(n,L3),0.);float NV=max(dot(n,V),.0);
vec3 diffuse=c*(vec3(.38,.46,.56)+vec3(.97,.965,.94)*d*.9+vec3(.47,.69,.92)*d2*.29+vec3(.57,.77,1.)*d3*.36);
float power=mix(170.,9.,uRough*uRough);float spec=pow(max(dot(n,normalize(L+V)),0.),power)*.80+pow(max(dot(n,normalize(L2+V)),0.),power*.65)*.31;
vec3 R=reflect(-V,n);vec3 env=mix(vec3(.06,.12,.21),vec3(.79,.90,.98),smoothstep(-.35,.85,R.y));float f=.04+.96*pow(1.-NV,5.);
float strip=pow(max(dot(R,normalize(vec3(-.65,.85,1.))),0.),34.)*.85+pow(max(dot(R,normalize(vec3(1.,.8,.15))),0.),50.)*.24;
vec3 result=diffuse*(1.-uMetal*.65)+env*(uMetal*.32+f*.12)+vec3(spec)*mix(.20,.62,uMetal)+vec3(strip)*(.10+uMetal*.20);
if(uMode>1.5&&uMode<2.5){float sheen=exp(-pow((vLocal.y-.23-vLocal.x*.19)/.10,2.))*smoothstep(.60,-.30,vLocal.x);result=diffuse*.62+env*(.016+f*.042)+vec3(spec)*.025+vec3(strip)*.009+vec3(.006,.01,.015)*sheen;}
if(uMode>2.5&&uMode<3.5){float flow=pow(max(0.,sin(vUv.x*21.-uTime*1.3)),14.);result+=c*(.6+flow*.6);alpha*=.60+.4*pow(1.-NV,2.);}
result+=c*uEmission;result=result/(result+vec3(.32));result=pow(result,vec3(1./2.2));frag=vec4(result,alpha);}`;
class Renderer{
constructor(canvas){
 this.canvas=canvas;
 this.gl=canvas.getContext('webgl2',{alpha:true,antialias:true,premultipliedAlpha:true,powerPreference:'low-power'});
 if(!this.gl)throw Error('WebGL2 unavailable');
 this.uploaded=new Map();this.textures=new Map();this.program=null;this.drawCalls=0;this.disposed=false;
 const gl=this.gl,shaders=[];this.shaderHandles=shaders;
 try{
  const compile=(type,src)=>{const shader=gl.createShader(type);if(!shader)throw Error('Shader allocation failed');shaders.push(shader);gl.shaderSource(shader,src);gl.compileShader(shader);if(!gl.getShaderParameter(shader,gl.COMPILE_STATUS))throw Error(gl.getShaderInfoLog(shader));return shader;};
  const v=compile(gl.VERTEX_SHADER,VS),f=compile(gl.FRAGMENT_SHADER,FS);
  this.program=gl.createProgram();if(!this.program)throw Error('Program allocation failed');gl.attachShader(this.program,v);gl.attachShader(this.program,f);gl.linkProgram(this.program);if(!gl.getProgramParameter(this.program,gl.LINK_STATUS))throw Error(gl.getProgramInfoLog(this.program));
  this.u={};for(const s of ['Model','VP','Color','Camera','Rough','Metal','Emission','Alpha','Mode','Time','Texture','VisorFX'])this.u[s]=gl.getUniformLocation(this.program,'u'+s);
  this.a={};for(const s of ['Position','Normal','Uv'])this.a[s]=gl.getAttribLocation(this.program,'a'+s);
  gl.enable(gl.DEPTH_TEST);gl.disable(gl.CULL_FACE);gl.enable(gl.BLEND);gl.blendFuncSeparate(gl.SRC_ALPHA,gl.ONE_MINUS_SRC_ALPHA,gl.ONE,gl.ONE_MINUS_SRC_ALPHA);
 }catch(error){this.dispose();throw error;}finally{for(const shader of shaders){if(this.program)gl.detachShader(this.program,shader);gl.deleteShader(shader);}}
}
bindTexture(mat){
 const gl=this.gl,source=mat.textureCanvas;if(!source)return;
 let entry=this.textures.get(source);
 if(!entry){const handle=gl.createTexture();if(!handle)throw Error('Texture allocation failed');entry={handle,version:-1};this.textures.set(source,entry);}
 gl.activeTexture(gl.TEXTURE0);gl.bindTexture(gl.TEXTURE_2D,entry.handle);
 if(entry.version!==mat.textureVersion){
  gl.pixelStorei(gl.UNPACK_FLIP_Y_WEBGL,true);
  gl.texImage2D(gl.TEXTURE_2D,0,gl.RGBA,gl.RGBA,gl.UNSIGNED_BYTE,source);
  gl.pixelStorei(gl.UNPACK_FLIP_Y_WEBGL,false);
  gl.texParameteri(gl.TEXTURE_2D,gl.TEXTURE_MIN_FILTER,gl.LINEAR);
  gl.texParameteri(gl.TEXTURE_2D,gl.TEXTURE_MAG_FILTER,gl.LINEAR);
  gl.texParameteri(gl.TEXTURE_2D,gl.TEXTURE_WRAP_S,gl.CLAMP_TO_EDGE);
  gl.texParameteri(gl.TEXTURE_2D,gl.TEXTURE_WRAP_T,gl.CLAMP_TO_EDGE);
  entry.version=mat.textureVersion;
 }
 gl.uniform1i(this.u.Texture,0);
}
setSize(w,h,dpr=1){w=Math.max(1,Math.round(w*dpr));h=Math.max(1,Math.round(h*dpr));if(this.canvas.width!==w||this.canvas.height!==h){this.canvas.width=w;this.canvas.height=h}}
upload(g){
 if(this.uploaded.has(g))return this.uploaded.get(g);
 const gl=this.gl,vao=gl.createVertexArray();if(!vao)throw Error('VAO allocation failed');
 const bufs=[];
 try{
  gl.bindVertexArray(vao);
  for(const [n,data,size] of [['Position',g.positions,3],['Normal',g.normals,3],['Uv',g.uvs,2]]){const b=gl.createBuffer();if(!b)throw Error('Buffer allocation failed');bufs.push(b);gl.bindBuffer(gl.ARRAY_BUFFER,b);gl.bufferData(gl.ARRAY_BUFFER,data,gl.STATIC_DRAW);gl.enableVertexAttribArray(this.a[n]);gl.vertexAttribPointer(this.a[n],size,gl.FLOAT,false,0,0);}
  const b=gl.createBuffer();if(!b)throw Error('Index allocation failed');bufs.push(b);gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER,b);gl.bufferData(gl.ELEMENT_ARRAY_BUFFER,g.indices,gl.STATIC_DRAW);
  const data={vao,bufs,count:g.indices.length,mode:g.mode==='line'?gl.LINES:gl.TRIANGLES};this.uploaded.set(g,data);return data;
 }catch(error){for(const buffer of bufs)gl.deleteBuffer(buffer);gl.deleteVertexArray(vao);throw error;}finally{gl.bindVertexArray(null);}
}
render(root,camera,time,visorFX=[0,0,0]){const gl=this.gl;gl.viewport(0,0,this.canvas.width,this.canvas.height);gl.depthMask(true);gl.clearColor(0,0,0,0);gl.clear(gl.COLOR_BUFFER_BIT|gl.DEPTH_BUFFER_BIT);gl.useProgram(this.program);const vp=mul(perspective(camera.fov,this.canvas.width/this.canvas.height),lookAt(camera.eye,camera.target));gl.uniformMatrix4fv(this.u.VP,false,vp);gl.uniform3fv(this.u.Camera,camera.eye);gl.uniform1f(this.u.Time,time);gl.uniform3fv(this.u.VisorFX,visorFX);const opaque=[],transparent=[];const visit=(node,parent)=>{if(!node.visible)return;node.world=mul(parent,compose(node.p,node.r,node.s));if(node.geo&&node.mat&&node.mat.alpha>.001)(node.mat.alpha<.995?transparent:opaque).push(node);for(const c of node.children)visit(c,node.world)};visit(root,identity());transparent.sort((a,b)=>{const p=a.world,q=b.world;return Math.hypot(q[12]-camera.eye[0],q[13]-camera.eye[1],q[14]-camera.eye[2])-Math.hypot(p[12]-camera.eye[0],p[13]-camera.eye[1],p[14]-camera.eye[2])});this.drawCalls=0;for(const list of [opaque,transparent]){gl.depthMask(list===opaque);for(const n of list){const g=this.upload(n.geo),m=n.mat;if(m.mode===5)this.bindTexture(m);gl.bindVertexArray(g.vao);gl.uniformMatrix4fv(this.u.Model,false,n.world);gl.uniform3fv(this.u.Color,m.color);gl.uniform1f(this.u.Rough,m.rough);gl.uniform1f(this.u.Metal,m.metal);gl.uniform1f(this.u.Emission,m.emit);gl.uniform1f(this.u.Alpha,m.alpha);gl.uniform1f(this.u.Mode,m.mode);gl.drawElements(g.mode,g.count,gl.UNSIGNED_SHORT,0);this.drawCalls++}}gl.bindVertexArray(null);gl.depthMask(true)}
dispose(){
 if(this.disposed)return;this.disposed=true;const gl=this.gl;
 gl.useProgram(null);gl.bindVertexArray(null);gl.bindBuffer(gl.ARRAY_BUFFER,null);gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER,null);gl.activeTexture(gl.TEXTURE0);gl.bindTexture(gl.TEXTURE_2D,null);
 const handles=(this.shaderHandles??[]).map(shader=>['Shader',shader]);
 for(const d of this.uploaded.values()){gl.deleteVertexArray(d.vao);handles.push(['VertexArray',d.vao]);for(const b of d.bufs){gl.deleteBuffer(b);handles.push(['Buffer',b]);}}
 for(const t of this.textures.values()){gl.deleteTexture(t.handle);handles.push(['Texture',t.handle]);}
 if(this.program){gl.deleteProgram(this.program);handles.push(['Program',this.program]);}
 this.releaseProof={checked:handles.length,alive:handles.filter(([kind,handle])=>gl['is'+kind]?.(handle)).length,contextLost:gl.isContextLost?.()??false};
 this.textures.clear();this.program=null;this.uploaded.clear();
}}
const POSES={
observe:{head:[-.06,-.15,-.06],root:[.04,.26,-.045],la:[-.72,-.12,-.60],le:[-.85,0,.18],lw:[-.24,-.2,-.2],ra:[-.15,.1,.28],re:[-.34,0,-.15],rw:[0,0,.1],lp:[-.15,.04,-.06],lk:[.12,0,0],rp:[.42,-.06,.13],rk:[.40,0,0]},
reflect:{head:[.16,-.15,-.12],root:[.03,.20,.055],la:[-.34,.02,-.17],le:[-1.5,.25,.68],lw:[-.15,0,-.40],ra:[-.05,.02,.15],re:[-2.1,-.2,-.62],rw:[-.4,.1,.15],lp:[.20,.04,-.1],lk:[.24,0,0],rp:[.10,-.04,.1],rk:[.15,0,0]},
validate:{head:[.13,.02,.02],root:[.07,.12,0],la:[-.62,-.10,-.30],le:[-1.0,.0,.10],lw:[-.55,0,-.55],ra:[-.62,.1,.30],re:[-1,0,-.10],rw:[-.55,0,.55],lp:[.10,0,-.1],lk:[.30,0,0],rp:[.23,0,.12],rk:[.3,0,0]},
feedback:{head:[-.10,.09,-.04],root:[-.12,.30,-.12],la:[-1.04,-.2,-.70],le:[-.4,.1,.3],lw:[-.25,-.20,-.15],ra:[.3,.1,.33],re:[-.9,0,-.22],rw:[0,0,.16],lp:[-.45,.05,-.07],lk:[.70,0,0],rp:[.63,-.05,.12],rk:[.63,0,0]}
};
const THEMES={sky:{accent:'#77cfff',core:'#31d6ff',light:'#16ceff',bright:'#baf9ff',ribbon:'#7edfff',css:'#239fdc'},glacier:{accent:'#88dcd9',core:'#6df1d8',light:'#87f8e8',bright:'#cffff4',ribbon:'#91eadc',css:'#199e9a'},iris:{accent:'#b2b9fc',core:'#bdc4ff',light:'#afc1ff',bright:'#e0e8ff',ribbon:'#c5caff',css:'#777fcb'},silver:{accent:'#b9ccd9',core:'#c7e7ff',light:'#d6ecff',bright:'#f0faff',ribbon:'#c9e5f3',css:'#6d95af'}};
function applyPose(dt,snap=false,at=time,override=state){if(!robot||!scene)return;updateVisor(override);const key=override==='waiting'?'reflect':override,target=POSES[key],k=snap?1:1-Math.exp(-dt*4.7);for(const name of Object.keys(livePose))for(let i=0;i<3;i++)livePose[name][i]=mix(livePose[name][i],target[name][i],k);const move=reduced?0:1, wave=Math.sin(at*1.3),breath=Math.sin(at*2.0);robot.root.r=[...livePose.root];robot.root.r[1]+=currentYaw+Math.sin(at*.6)*.035*move;robot.root.p[1]=.04+wave*.09*move;robot.root.p[0]=Math.sin(at*.57)*.025*move;robot.torso.s=[1,1+breath*.006*move,1];robot.head.r=[...livePose.head];robot.head.r[1]+=Math.sin(at*.92)*.065*move;robot.head.r[2]+=Math.sin(at*.70)*.018*move;
for(let i=0;i<2;i++){const a=robot.arms[i],side=i===0?'l':'r';a.pivot.r=[...livePose[side+'a']];a.elbow.r=[...livePose[side+'e']];a.wrist.r=[...livePose[side+'w']];a.pivot.r[2]+=Math.sin(at*1.8+i)*.026*move;a.elbow.r[0]+=Math.sin(at*1.2+i*.8)*.055*move;for(let j=0;j<3;j++){a.fingers[j].r[0]=key==='reflect'?-.46:(key==='validate'?-.22:-.09-Math.sin(at*1.5+j*.65)*.06*move);a.fingers[j].r[2]=(j-1)*.09}const l=robot.legs[i];l.p.r=[...livePose[side+'p']];l.k.r=[...livePose[side+'k']];l.p.r[0]+=Math.sin(at*1.3+i)*.05*move;l.k.r[0]+=Math.cos(at*1.3+i)*.065*move;l.foot.r[0]=-.10;}
const blinkPhase=at%5.8,blink=!reduced&&blinkPhase>5.53?1-Math.sin((blinkPhase-5.53)/.27*Math.PI)*.92:1;robot.eyes.forEach(e=>{e.s[1]=blink*(key==='reflect'?.64:1)});
robot.layers.forEach((l,i)=>{l.r[1]=.62+(key==='validate'?Math.sin(at*.85+i*.25)*.14:Math.sin(at*.6)*.05)*move;l.p[1]=(i-1.5)*(.128+(key==='feedback'?.020*Math.sin(at*1.4):.009*Math.sin(at*1.6+i)) *move+.128*(1-move));});
scene.orbit.r[1]=at*.12*move;scene.orbit.r[2]=-.22+Math.sin(at*.4)*.03*move;scene.halo.r[1]=at*.07*move;scene.orbit.visible=orbitOn;scene.halo.visible=orbitOn;scene.particles.forEach((p,i)=>{const a=i/19*TAU+at*(.09+(i%3)*.012)*move,r=1.75+(i%4)*.09;p.p=[Math.cos(a)*r,-1.05+(i%7)*.43+Math.sin(at+i)*.06*move,Math.sin(a)*.74-.14];p.r=[at*.2+i,at*.22+i,at*.1];p.visible=orbitOn});
scene.dataCard.visible=key==='observe'||key==='reflect';scene.dataCard.r[1]=.3+Math.sin(at*.6)*.1*move;scene.dataCard.p[1]=.44+Math.sin(at*1.2)*.065*move;scene.dataCard.p[0]=key==='reflect'?-1.04:-1.2;
scene.candidate.visible=key==='validate'||key==='feedback';scene.candidate.p=key==='validate'?[.0,-.08,1.18]:[.96,.26,.88];scene.candidate.r[1]=at*.32*move;scene.cubes.forEach((c,i)=>{c.p[1]=(i-2)*(.115+.018*Math.sin(at*1.5)*move);c.r[1]=Math.PI/4+Math.sin(at+i*.42)*.12*move});}

let livePose=JSON.parse(JSON.stringify(POSES.observe));
try{renderer=new Renderer(canvas);robot=buildRobot();if(!robot.visor)throw Error('Visor canvas unavailable');scene=buildScene(robot);}catch(error){renderer?.dispose();throw error;}
const fxModes=Object.freeze({steady:0,sweep:1,flow:2,breath:3});
let theme='sky',renders=0;
return Object.freeze({
 renderFrame(options={}){
  if(disposed)throw Error('Scene is disposed');
  const pose=options.pose??'observe';if(pose!=='waiting'&&!Object.hasOwn(POSES,pose))throw TypeError('Unknown pose');
  const label=options.label??'未连接';if(typeof label!=='string'||Array.from(label).length<1||Array.from(label).length>4)throw TypeError('Expected short display label');
  const at=options.time??0;if(!Number.isFinite(at)||at<0)throw TypeError('Invalid display time');
  const palette=options.theme??'sky';if(!Object.hasOwn(THEMES,palette))throw TypeError('Unknown palette');
  const effect=options.effect??'steady';if(!Object.hasOwn(fxModes,effect))throw TypeError('Unknown visor effect');
  const width=options.width??320,height=options.height??340,dpr=options.dpr??1;
  if(!Number.isFinite(width)||!Number.isFinite(height)||width<=0||height<=0||!Number.isFinite(dpr)||dpr<=0)throw TypeError('Invalid frame size');
  const yaw=options.yaw??0;if(!Number.isFinite(yaw))throw TypeError('Invalid yaw');
  state=pose;displayLabel=label;time=at;reduced=options.reducedMotion??true;currentYaw=clamp(yaw,-1.9,1.9);orbitOn=options.orbit??true;
  if(theme!==palette){theme=palette;const p=THEMES[theme];for(const key of ['accent','core','light','bright','ribbon'])mats[key].color=hex(p[key]);mats.line.color=hex(p.bright);mats.glow.color=hex(p.light);if(robot.visor)robot.visor.mat.color=hex(p.bright);}
  applyPose(options.delta??0,options.snap??true,at,state);
  if(!options.artifacts){scene.dataCard.visible=false;scene.candidate.visible=false;}
  renderer.setSize(width,height,Math.min(dpr,1.65));
  const ratio=width/height,focus=options.focus??false;
  const camera=focus?{eye:[0,1.48,ratio<.72?5.1:3.8],target:[0,1.08,.15],fov:.60}:{eye:[0,.64,ratio<.72?8.65:7.0],target:[0,.15,0],fov:.69};
  const stationary=reduced||options.paused||pose==='waiting'||effect==='steady';
  const v=robot.visor,age=Math.max(0,time-(v?.changedAt??-10));
  const reveal=!stationary&&age<.42?Math.sin(age/.42*Math.PI):0;
  renderer.render(scene.root,camera,time,[stationary?0:fxModes[effect],Math.max(0,time-(v?.phaseStart??0)),reveal]);renders++;
 },
 invalidateFont(){if(!disposed&&robot?.visor)robot.visor.lastLabel=null;},
 getDiagnostics(){return Object.freeze({disposed,renders,theme,text:robot?.visor?.lastLabel??'',textureRevision:robot?.visor?.mat.textureVersion??0,textureCount:renderer?.textures.size??0,geometryCount:renderer?.uploaded.size??0,drawCalls:renderer?.drawCalls??0,releaseProof:renderer?.releaseProof??null});},
 capturePixels(){if(disposed)throw Error('Scene is disposed');const gl=renderer.gl,pixels=new Uint8Array(canvas.width*canvas.height*4);gl.readPixels(0,0,canvas.width,canvas.height,gl.RGBA,gl.UNSIGNED_BYTE,pixels);return {width:canvas.width,height:canvas.height,pixels};},
 dispose({releaseContext=false}={}){if(!disposed){disposed=true;renderer?.dispose();cache.clear();robot=null;scene=null;}if(releaseContext)renderer?.gl.getExtension('WEBGL_lose_context')?.loseContext();}
});
}
