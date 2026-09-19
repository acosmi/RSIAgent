-- Persistent root monetary budget and per-call state machine.
-- One billing_scope names exactly one experiment root across namespaces.
CREATE TABLE root_budgets (
 billing_scope TEXT PRIMARY KEY,
 root_budget_id TEXT NOT NULL UNIQUE,
 authorizing_namespace TEXT NOT NULL,
 currency TEXT NOT NULL,
 pricing_version TEXT NOT NULL,
 payment_subject TEXT NOT NULL,
 authorization_receipt_digest TEXT NOT NULL,
 per_call_cap_micros INTEGER NOT NULL CHECK(per_call_cap_micros > 0),
 total_limit_micros INTEGER NOT NULL CHECK(total_limit_micros > 0),
 spent_micros INTEGER NOT NULL DEFAULT 0 CHECK(spent_micros >= 0),
 reserved_micros INTEGER NOT NULL DEFAULT 0 CHECK(reserved_micros >= 0),
 concurrency_limit INTEGER NOT NULL DEFAULT 1 CHECK(concurrency_limit = 1),
 stopped INTEGER NOT NULL DEFAULT 0 CHECK(stopped IN (0,1)),
 stop_reason TEXT,
 stop_committed_at INTEGER,
 created_at INTEGER NOT NULL CHECK(created_at >= 0),
 CHECK(per_call_cap_micros <= total_limit_micros),
 CHECK((stopped = 0 AND stop_reason IS NULL AND stop_committed_at IS NULL)
    OR (stopped = 1 AND stop_reason IS NOT NULL AND stop_committed_at IS NOT NULL))
);

CREATE TABLE root_budget_namespaces (
 billing_scope TEXT NOT NULL REFERENCES root_budgets(billing_scope) ON DELETE RESTRICT,
 namespace TEXT NOT NULL,
 PRIMARY KEY(billing_scope, namespace)
);

CREATE TABLE root_budget_dispatch_groups (
 billing_scope TEXT NOT NULL REFERENCES root_budgets(billing_scope) ON DELETE RESTRICT,
 dispatch_group_id TEXT NOT NULL,
 owner_namespace TEXT NOT NULL,
 stopped INTEGER NOT NULL DEFAULT 0 CHECK(stopped IN (0,1)),
 stop_reason TEXT,
 stop_committed_at INTEGER,
 created_at INTEGER NOT NULL CHECK(created_at >= 0),
 PRIMARY KEY(billing_scope, dispatch_group_id),
 CHECK((stopped = 0 AND stop_reason IS NULL AND stop_committed_at IS NULL)
    OR (stopped = 1 AND stop_reason IS NOT NULL AND stop_committed_at IS NOT NULL))
);

CREATE TABLE root_budget_calls (
 billing_scope TEXT NOT NULL REFERENCES root_budgets(billing_scope) ON DELETE RESTRICT,
 call_id TEXT NOT NULL,
 dispatch_group_id TEXT NOT NULL,
 namespace TEXT NOT NULL,
 stage TEXT NOT NULL,
 actual_input_digest TEXT NOT NULL,
 request_artifact_schema TEXT,
 request_artifact_digest TEXT,
 request_artifact_body TEXT CHECK(request_artifact_body IS NULL OR json_valid(request_artifact_body)),
 reserved_micros INTEGER NOT NULL CHECK(reserved_micros > 0),
 state TEXT NOT NULL CHECK(state IN ('reserved','dispatched','uncertain','finalized','released','cancelled')),
 lease_token TEXT NOT NULL,
 lease_epoch INTEGER NOT NULL CHECK(lease_epoch > 0),
 lease_until INTEGER NOT NULL CHECK(lease_until >= 0),
 dispatch_id TEXT,
 provider_request_id TEXT,
 usage_record_id TEXT,
 output_digest TEXT,
 actual_cost_micros INTEGER CHECK(actual_cost_micros >= 0),
 actual_currency TEXT,
 actual_pricing_version TEXT,
 execution_provenance TEXT CHECK(execution_provenance IS NULL OR execution_provenance IN ('fixture','external_provider')),
 actual_model_digest TEXT,
 transport_artifact_schema TEXT,
 transport_artifact_digest TEXT,
 transport_artifact_body TEXT CHECK(transport_artifact_body IS NULL OR json_valid(transport_artifact_body)),
 response_artifact_schema TEXT,
 response_artifact_digest TEXT,
 response_artifact_body TEXT CHECK(response_artifact_body IS NULL OR json_valid(response_artifact_body)),
 response_usable INTEGER CHECK(response_usable IS NULL OR response_usable IN (0,1)),
 response_block_reason TEXT,
 execution_closed INTEGER NOT NULL DEFAULT 1 CHECK(execution_closed IN (0,1)),
 execution_closed_at INTEGER,
 execution_close_reason TEXT,
 terminal_reason TEXT,
 created_at INTEGER NOT NULL CHECK(created_at >= 0),
 dispatched_at INTEGER,
 finalized_at INTEGER,
 PRIMARY KEY(billing_scope, call_id),
 FOREIGN KEY(billing_scope,dispatch_group_id)
   REFERENCES root_budget_dispatch_groups(billing_scope,dispatch_group_id) ON DELETE RESTRICT,
 UNIQUE(dispatch_id),
 UNIQUE(usage_record_id),
 CHECK((state = 'reserved' AND dispatch_id IS NULL AND dispatched_at IS NULL AND execution_closed = 1)
    OR state <> 'reserved'),
 CHECK((execution_closed = 0 AND execution_closed_at IS NULL AND execution_close_reason IS NULL)
    OR (execution_closed = 1 AND (dispatch_id IS NULL OR (execution_closed_at IS NOT NULL AND execution_close_reason IS NOT NULL)))),
 CHECK((request_artifact_schema IS NULL AND request_artifact_digest IS NULL AND request_artifact_body IS NULL)
    OR (request_artifact_schema IS NOT NULL AND request_artifact_digest IS NOT NULL AND request_artifact_body IS NOT NULL)),
 CHECK((response_artifact_schema IS NULL AND response_artifact_digest IS NULL AND response_artifact_body IS NULL
        AND response_usable IS NULL AND response_block_reason IS NULL)
    OR (response_artifact_schema IS NOT NULL AND response_artifact_digest IS NOT NULL AND response_artifact_body IS NOT NULL
        AND response_usable IS NOT NULL)),
 CHECK((transport_artifact_schema IS NULL AND transport_artifact_digest IS NULL AND transport_artifact_body IS NULL)
    OR (transport_artifact_schema IS NOT NULL AND transport_artifact_digest IS NOT NULL AND transport_artifact_body IS NOT NULL))
);

CREATE INDEX root_budget_calls_active
 ON root_budget_calls(billing_scope, state, execution_closed);
CREATE INDEX root_budget_calls_namespace
 ON root_budget_calls(namespace, billing_scope, call_id);

CREATE TABLE root_budget_events (
 seq INTEGER PRIMARY KEY AUTOINCREMENT,
 billing_scope TEXT NOT NULL REFERENCES root_budgets(billing_scope) ON DELETE RESTRICT,
 call_id TEXT,
 event_kind TEXT NOT NULL,
 event_at INTEGER NOT NULL CHECK(event_at >= 0),
 details TEXT NOT NULL CHECK(json_valid(details))
);
CREATE INDEX root_budget_events_scope
 ON root_budget_events(billing_scope, seq);
