# process-supervision

`process-supervision` runs CI automation child processes under bounded
supervision. It is a library only. Its normal dependencies are thiserror, libc
on Unix, and windows-sys on Windows.

It owns:

- Supervision of cooperative child trees (`supervise`, `supervise_raw`,
  `supervise_with_probe`, `supervise_projected`, `supervise_inherited`):
  deadlines, readiness probes, bounded output capture, secret redaction
  (`Value::Secret`), and cleanup. Unix children keep their inherited process
  group. Windows children start suspended and join a kill-on-close job.
- `retained`: supervision of several long-lived members under one coordinator,
  with recovery and shutdown reporting.
- `interrupt`: the process-wide SIGINT/SIGTERM (or console control) scope whose
  `Cancellation` stops supervised children.

Run the library tests with `just process-supervision-test`.
