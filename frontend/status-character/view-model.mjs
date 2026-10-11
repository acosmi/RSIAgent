// Presentation derives only from the existing management-job contract.
const operations = new Set(['experiment.register','evaluation.start','evaluation.status','exploration.start','replay.run','curriculum.step','meta.start']);
const states = new Set(['queued','running','draining','succeeded','failed','cancelled','blocked']);
const terminal = new Set(['succeeded','failed','cancelled','blocked']);
const view = (label, pose='observe', animated=false, reason=null) => Object.freeze({label,pose,animated,artifacts:pose==='validate'&&animated,reason});

export function mapManagementSnapshot(job, expectedId) {
  if (!job || typeof job !== 'object' || Array.isArray(job)
      || typeof expectedId !== 'string' || !expectedId
      || job.id !== expectedId || job.schema_version !== 'rsia.management_job.v1'
      || !operations.has(job.operation) || !states.has(job.state)
      || !Number.isSafeInteger(job.generation) || job.generation < 0
      || typeof job.cancel_requested !== 'boolean') {
    return view('不可用','observe',false,'invalid_or_mismatched_snapshot');
  }
  if (!terminal.has(job.state) && job.cancel_requested) return view('取消中');
  if (job.state === 'queued') return view('排队中');
  if (job.state === 'draining') return view('收尾中');
  if (job.state === 'cancelled') return view('已取消');
  if (job.state === 'failed') return view('异常');
  if (job.state === 'blocked') return view('受阻中');
  if (job.state === 'running') return view('执行中','observe',true);

  // A completed status-read job carries a refreshed ticket lifecycle. It does
  // not grant a verdict or make evaluation.start mean execution/acceptance.
  if (job.operation === 'evaluation.status') {
    const r=job.result;
    if (!r || r.result !== 'evaluation_status' || typeof r.ticket_id !== 'string' || !r.ticket_id
        || !Number.isSafeInteger(r.planned_targets) || r.planned_targets < 0
        || typeof r.has_formal_terminal !== 'boolean') return view('不可用','observe',false,'incomplete_evaluation_status');
    const labels={reserved:'待执行',running:'验证中',ready_for_final:'待收口',stop_requested:'停止中',draining:'收尾中',early_stopped:'已早停',complete_batch:'已结束',invalid:'异常'};
    if (!Object.hasOwn(labels,r.lifecycle)) return view('不可用','observe',false,'unknown_ticket_lifecycle');
    return view(labels[r.lifecycle],r.lifecycle==='running'?'validate':'observe',r.lifecycle==='running');
  }
  return view('已结束');
}

// This counter invalidates transport callbacks; it is not a backend revision.
export function createScopeGate() {
  let selection=null,token=0,disposed=false,latestGeneration=null;
  return Object.freeze({
    select(connection,id){if(disposed)throw Error('Gate is disposed');selection={connection,id};latestGeneration=null;return ++token;},
    accept(callbackToken,connection,job){
      if(disposed||callbackToken!==token||!selection||selection.connection!==connection) return null;
      const value=mapManagementSnapshot(job,selection.id);
      if(value.reason) return value;
      if(latestGeneration!==null&&job.generation<latestGeneration)return null;
      latestGeneration=job.generation;return value;
    },
    dispose(){disposed=true;selection=null;latestGeneration=null;++token;}
  });
}
