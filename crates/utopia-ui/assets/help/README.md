# Help screenshots

These PNGs are embedded in the binary by `crates/utopia-ui/src/help.rs` and shown in
Settings → Help. Regenerate them with:

```bash
cargo run -p xtask -- help-shots
```

That builds the app, seeds a mock graph in `target/help-shots/Demo.utopia` (a small
project, a person and a few days of notes that link them), drives the real window with
`UTOPIA_SHOT_SCRIPT` and downscales each capture to 1200 px wide.

The app captures its own window, so no Screen Recording permission is needed. The Settings
window is a separate window and the shot script drives one window only, which is why no
Help topic shows a picture of Settings.

Review the captures by eye after regenerating: colours, typography and layout are not
covered by tests (see AGENTS.md, "Known limits").
