import http from 'node:http';
import {readFile,realpath} from 'node:fs/promises';
import {fileURLToPath} from 'node:url';
import path from 'node:path';

const appRoot=fileURLToPath(new URL('.',import.meta.url));
const files=new Map([
  ['/','index.html'],['/index.html','index.html'],['/character.css','character.css'],['/page.css','page.css'],['/entry.mjs','entry.mjs'],['/character.mjs','character.mjs'],['/v3-scene.mjs','v3-scene.mjs'],['/view-model.mjs','view-model.mjs'],['/status-source.mjs','status-source.mjs']
]);
const qaFiles=new Map([['/qa/','qa/index.html'],['/qa/index.html','qa/index.html'],['/qa/fixture.mjs','qa/fixture.mjs'],['/qa/fixture.css','qa/fixture.css']]);
const media={'.html':'text/html; charset=utf-8','.mjs':'text/javascript; charset=utf-8','.css':'text/css; charset=utf-8'};
const boundedText=async response=>{
  const chunks=[];let size=0;
  for await(const chunk of response.body??[]){size+=chunk.length;if(size>65536)throw Error('Response too large');chunks.push(chunk);}
  return Buffer.concat(chunks).toString('utf8');
};
export function projectJob(job){
  if(!job||typeof job!=='object'||Array.isArray(job))return null;
  const out={};for(const key of ['schema_version','id','operation','state','generation','cancel_requested'])if(Object.hasOwn(job,key))out[key]=job[key];
  if(job.operation==='evaluation.status'&&job.result&&typeof job.result==='object'){
    out.result={};for(const key of ['result','ticket_id','lifecycle','planned_targets','has_formal_terminal'])if(Object.hasOwn(job.result,key))out.result[key]=job.result[key];
  }
  return out;
}
export async function createBridge({port=0,qa=false,connection=null,fetchBackend=fetch}={}){
  if(!Number.isSafeInteger(port)||port<0||port>65535)throw TypeError('Invalid port');
  let backend=null;
  if(connection){
    const url=new URL(connection.url);
    if(url.protocol!=='http:'||!['127.0.0.1','localhost'].includes(url.hostname)||url.username||url.password||url.pathname!=='/'||url.search||url.hash)throw TypeError('Use an explicit local backend origin');
    if(typeof connection.jobId!=='string'||!/^[A-Za-z0-9_-]{1,160}$/.test(connection.jobId)||typeof connection.token!=='string'||!connection.token||/[\r\n]/.test(connection.token))throw TypeError('A fixed existing job and credential are required');
    backend={origin:url.origin,jobId:connection.jobId,token:connection.token};
  }
  const whitelist=new Map([...files,...(qa?qaFiles:[])]),root=await realpath(appRoot);
  const server=http.createServer(async(req,res)=>{
    const origin=`http://127.0.0.1:${server.address().port}`;
    res.setHeader('Cache-Control','no-store');res.setHeader('X-Content-Type-Options','nosniff');res.setHeader('Referrer-Policy','no-referrer');
    res.setHeader('Content-Security-Policy',"default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; font-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'");
    const send=(status,body,type='application/json; charset=utf-8')=>{res.writeHead(status,{'Content-Type':type});res.end(req.method==='HEAD'?undefined:typeof body==='string'||Buffer.isBuffer(body)?body:JSON.stringify(body));};
    if(req.headers.host!==`127.0.0.1:${server.address().port}`||(req.headers.origin&&req.headers.origin!==origin)||req.headers['sec-fetch-site']==='cross-site'){send(403,{code:'origin_rejected'});return;}
    if(!['GET','HEAD'].includes(req.method)){res.setHeader('Allow','GET, HEAD');send(405,{code:'read_only'});return;}
    const route=req.url;
    if(typeof route!=='string'||!route.startsWith('/')||route.includes('?')||route.includes('%')||route.includes('..')||route.includes('\\')){send(404,{code:'not_found'});return;}
    if(route==='/connection'){send(200,{configured:Boolean(backend),job_id:backend?.jobId??null});return;}
    if(route==='/status'){
      if(!backend){send(503,{code:'not_configured'});return;}
      if(req.method==='HEAD'){send(200,'');return;}
      const controller=new AbortController(),deadline=setTimeout(()=>controller.abort(),4000),abort=()=>controller.abort();req.on('aborted',abort);
      try{
        const response=await fetchBackend(`${backend.origin}/v1/manage/jobs/${encodeURIComponent(backend.jobId)}`,{method:'GET',signal:controller.signal,redirect:'error',headers:{Authorization:`Bearer ${backend.token}`,Accept:'application/json'}});
        if(!response.ok){send([401,403,404,409].includes(response.status)?response.status:502,{code:'status_unavailable'});return;}
        const job=projectJob(JSON.parse(await boundedText(response)));if(!job)throw Error('Invalid response');send(200,job);
      }catch{if(!res.destroyed)send(502,{code:'status_unavailable'});}finally{clearTimeout(deadline);req.off('aborted',abort);}
      return;
    }
    const relative=whitelist.get(route);if(!relative){send(404,{code:'not_found'});return;}
    try{
      const resolved=await realpath(path.join(root,relative));if(!resolved.startsWith(root+path.sep))throw Error('Static file escapes root');
      send(200,await readFile(resolved),media[path.extname(relative)]);
    }catch{send(404,{code:'not_found'});}
  });
  await new Promise((resolve,reject)=>{server.once('error',reject);server.listen(port,'127.0.0.1',()=>{server.off('error',reject);resolve();});});
  return Object.freeze({origin:`http://127.0.0.1:${server.address().port}`,server,close:()=>new Promise((resolve,reject)=>server.close(error=>error?reject(error):resolve()))});
}
if(process.argv[1]&&path.resolve(process.argv[1])===fileURLToPath(import.meta.url)){
  const args=process.argv.slice(2),get=flag=>{const index=args.indexOf(flag);return index<0?null:args[index+1];};
  const live=args.includes('--connect');
  if(live&&args.includes('--preview'))throw Error('Choose one mode');
  const connection=live?{url:get('--backend')??'http://127.0.0.1:7788',jobId:get('--job'),token:process.env.RSIA_HTTP_TOKEN}:null;
  const bridge=await createBridge({port:Number(get('--port')??0),qa:args.includes('--qa'),connection});
  process.stdout.write(JSON.stringify({pid:process.pid,origin:bridge.origin,mode:live?'fixed_job_read':'disconnected_preview',qa:args.includes('--qa')})+'\n');
  for(const signal of ['SIGINT','SIGTERM'])process.once(signal,async()=>{await bridge.close();process.exit(0);});
}
