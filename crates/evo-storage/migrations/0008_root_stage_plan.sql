-- Root stage plan binding (plan §3.5.1, V103). Append-only versions and the
-- immutable control/call relations that point at the active version. Every
-- table name starts with `root_budget` so recovery protects it under the
-- existing `LIKE 'root_budget%'` selector.
CREATE TABLE root_budget_stage_plan_bindings (
 billing_scope TEXT NOT NULL REFERENCES root_budgets(billing_scope) ON DELETE RESTRICT,
 seq INTEGER NOT NULL CHECK(seq > 0),
 plan_schema TEXT NOT NULL,
 plan_digest TEXT NOT NULL,
 plan_body TEXT NOT NULL CHECK(json_valid(plan_body)),
 payer_id TEXT NOT NULL,
 root_total_micros INTEGER NOT NULL CHECK(root_total_micros > 0),
 currency TEXT NOT NULL,
 pricing_version TEXT NOT NULL,
 authorization_receipt_digest TEXT NOT NULL,
 authorizing_namespace TEXT NOT NULL,
 bound_by_actor TEXT NOT NULL,
 created_at INTEGER NOT NULL CHECK(created_at >= 0),
 PRIMARY KEY(billing_scope, seq)
);
CREATE INDEX root_budget_stage_plan_bindings_digest
 ON root_budget_stage_plan_bindings(billing_scope, plan_digest);

CREATE TABLE root_budget_stage_plan_controls (
 billing_scope TEXT NOT NULL REFERENCES root_budgets(billing_scope) ON DELETE RESTRICT,
 namespace TEXT NOT NULL,
 control_kind TEXT NOT NULL,
 control_id TEXT NOT NULL,
 control_digest TEXT NOT NULL,
 binding_seq INTEGER NOT NULL,
 plan_digest TEXT NOT NULL,
 created_at INTEGER NOT NULL CHECK(created_at >= 0),
 PRIMARY KEY(namespace, control_kind, control_id),
 FOREIGN KEY(billing_scope, binding_seq)
   REFERENCES root_budget_stage_plan_bindings(billing_scope, seq) ON DELETE RESTRICT
);
CREATE INDEX root_budget_stage_plan_controls_scope
 ON root_budget_stage_plan_controls(billing_scope, binding_seq);

CREATE TABLE root_budget_stage_plan_calls (
 billing_scope TEXT NOT NULL,
 call_id TEXT NOT NULL,
 namespace TEXT NOT NULL,
 stage TEXT NOT NULL,
 binding_seq INTEGER NOT NULL,
 plan_digest TEXT NOT NULL,
 course_producer_ref TEXT,
 created_at INTEGER NOT NULL CHECK(created_at >= 0),
 PRIMARY KEY(billing_scope, call_id),
 FOREIGN KEY(billing_scope, call_id)
   REFERENCES root_budget_calls(billing_scope, call_id) ON DELETE RESTRICT,
 FOREIGN KEY(billing_scope, binding_seq)
   REFERENCES root_budget_stage_plan_bindings(billing_scope, seq) ON DELETE RESTRICT
);
CREATE INDEX root_budget_stage_plan_calls_scope
 ON root_budget_stage_plan_calls(billing_scope, binding_seq);
