# citadel-app-sdk

This crate is for writing a Citadel app in Rust. You implement `App`, name it in `export_app!`,
and build for `wasm32-unknown-unknown`. The linker flags, the host-call model and `Pending` are
covered in the crate documentation (`cargo doc --open`, or the top of `src/lib.rs`).

## The lifecycle every app implements

The host drives one state machine. The table is `citadel_app_sdk_types::lifecycle`, and the
browser host and the reference host read the same table:

```text
Created -> Initialised (init returned Ok) -> Running (AppEvent::Init) <-> Suspended -> Closing (close) -> Closed
```

Your `App` implements all five lifecycle functions:

```rust
use citadel_app_sdk::{export_app, storage, App, AppContext, AppEvent, CloseContext, CloseReason, InitFailure, SuspendReason};

struct Board { edits: u32 }

impl App for Board {
    fn init(ctx: AppContext) -> Result<Self, InitFailure> {
        // ctx.size is the surface in CSS px; ctx.close_grace_ms is how long a close may take.
        Ok(Board { edits: 0 })
    }
    fn event(&mut self, event: AppEvent) {
        if let AppEvent::Resize { width, height, .. } = event { /* relayout */ }
    }
    fn suspend(&mut self, _reason: SuspendReason) { /* out of sight: no frames, no input */ }
    fn resume(&mut self) { /* back in sight */ }
    fn close(&mut self, _ctx: &CloseContext, _reason: CloseReason) {
        // Persist what you hold. The reply reaches you before the host ends the instance,
        // as long as it arrives within ctx.grace_ms.
        let _ = storage::user().set("state", format!("edits={}", self.edits).as_bytes());
    }
}

export_app!(Board);
```

`export_app!` exports `app_close`, which ABI 1 requires. A module without it is refused at
install with `missing_export:app_close`.

### What reaches you, and when

- **Initialised:** only `init` has run. The host has not started you yet, so it sends no frames
  or input.
- **Running:** every event and every frame.
- **Suspended:** the surface is out of view (`Hidden`) or the page is in the background
  (`Backgrounded`). The host sends no frames and no input. Replies, storage changes and
  internal-HTTP frames still arrive, because a suspended app is alive.
- **Closing:** `close` runs once. Only replies still arrive. A host call from now on reaches
  `storage` and `log` only; anything else is refused with `{unavailable: "closing"}`. A second
  `app_close` answers `not_running` and does not run `close` again.
- **Closed:** nothing runs again.

### The close grace

Your manifest must give `limits.close_grace_ms`, from 1 to 2000 ms (`MAX_CLOSE_GRACE_MS`). Your
`close` and the replies it waits on get that long. Then the host ends the instance, whether or
not `close` returned.

The grace bounds your cleanup. It is not a timeout to retry within. Write what you hold, log if
you must, and return. A `close` that never returns is cut off at the grace, and a panic in
`close` traps. Either way, the host still releases everything else the instance held.

`CloseReason` says why you are closing:

| reason | when |
|---|---|
| `Unmount` | the surface went away |
| `Uninstall` | the install was removed |
| `AccessRevoked` | the install no longer reaches this person |
| `HostShutdown` | the page is closing |
| `VersionUpdate` | another release replaces this one |
| `HostError` | the host had to stop you for a fault of its own |

On a close, the host also cancels your internal-HTTP streams with `Cancel { Closed }` and
withdraws your advertisement, so your callers learn at once.

The full contract, with what the host and the server do on every exit path, is
`docs/app-lifecycle.md` in the workspace repository.
