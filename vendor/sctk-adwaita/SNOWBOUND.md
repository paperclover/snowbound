# sctk-adwaita, vendored

Upstream: [sctk-adwaita](https://github.com/PolyMeilex/sctk-adwaita) 0.10.1 from
crates.io (the version winit resolves to in `Cargo.lock`), copied without its
`examples/`, `Cargo.lock` and CI files. The workspace `Cargo.toml` points
`[patch.crates-io]` here.

`Cargo.toml` gains a `[lints.rust]` table silencing a lint newer than the
release, since Cargo does not cap a path dependency's warnings. The one source
change drops the 1 px border line under the header bar, so the title
bar flows into Snowbound's toolbar as it does in a GTK 4 app:

```diff
--- a/src/lib.rs
+++ b/src/lib.rs
@@ fn draw_headerbar_bg(
         None,
     );
 
-    pixmap.fill_rect(
-        Rect::from_xywh(0., h - 1., w, h)?,
-        &colors.border_paint(),
-        Transform::identity(),
-        None,
-    );
-
     Some(())
 }
```

To bump: copy the new release's `src/`, `Cargo.toml`, `LICENSE`, `README.md`
and `CHANGELOG.md` over this folder, reapply the diff, and update the version
above. The patch applies only while the version here satisfies winit's
requirement; `cargo tree -i sctk-adwaita` shows the path source when it does.
