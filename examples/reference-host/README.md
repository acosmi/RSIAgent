# Reference host (E04)

Read-only example host used for isolation tests. It does not grant a sandbox.

- Workspace is a temporary directory supplied per run.
- Network is deny-by-default.
- Code execution remains `disabled`.
- Missing sandbox returns `sandbox_unavailable`; the host shell is not a fallback.
