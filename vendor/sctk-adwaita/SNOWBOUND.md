# sctk-adwaita, vendored

Upstream: [sctk-adwaita](https://github.com/PolyMeilex/sctk-adwaita) 0.10.1 from
crates.io (the version winit resolves to in `Cargo.lock`), copied without its
`examples/`, `Cargo.lock` and CI files. The workspace `Cargo.toml` points
`[patch.crates-io]` here.

`Cargo.toml` gains a `[lints.rust]` table silencing a lint newer than the
release, since Cargo does not cap a path dependency's warnings, and takes
tiny-skia 0.12, as resvg does, in place of 0.11, whose API it shares. The first source
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

The second change serves GNOME, where Snowbound cuts its window's bottom
corners as libadwaita does. With `LIBADWAITA_EDGE` set, the frame draws
libadwaita 1.7's `window.csd` edge in place of its own border
(`draw_libadwaita_edge`): the 15 px `WINDOW_RADIUS`, the box shadows focused
and not with their 1 px ring, and the 7 % white outline 1 px inside the
edge. The header keeps the body's width, the sides overlap the body by the
outline and the bottom by the radius, so the frame paints the outline and the
bottom corners' shadow over the application's pixels.

The third change makes the header bar zero height, so Snowbound's toolbar row
is the title bar, as a GTK 4 header bar holding tools is (`theme.rs`):

```diff
-pub(crate) const HEADER_SIZE: u32 = 35;
+pub(crate) const HEADER_SIZE: u32 = 0;
```

Around it: `FRAMED` is set once a frame is made, which tells Snowbound to draw
the title bar and the window's buttons itself; the empty header part is not
drawn; the top part draws a border line as the bottom does; and with
`LIBADWAITA_EDGE` the top part reaches down over the body's top corners by the
radius, as the bottom reaches up, owning them from the sides, since Snowbound
cuts those corners too.

To bump: copy the new release's `src/`, `Cargo.toml`, `LICENSE`, `README.md`
and `CHANGELOG.md` over this folder, reapply the three changes, and update the version
above. The patch applies only while the version here satisfies winit's
requirement; `cargo tree -i sctk-adwaita` shows the path source when it does.
