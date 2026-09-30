<h1>
  <img src="crates/snowbound/assets/icon/Snowbound-SnowLeopard.png" width="32">
  Snowbound — Freeform note taking
</h1>

Type and draw notes on any platform, while maintaining ownership of your data.
Snowbound does not use a typical markdown or structured note format, but rather
uses rich text on a free canvas. Unlike other infinite canvas apps, our canvas
feels more like a standard text editor software as textboxes automatically
create and resize as you would expect. In addition, Snowbound supports
pen and drawing tools [(WIP)](https://shale.paperclover.net/snowbound/issues/21), recording audio and video
[(WIP)](https://shale.paperclover.net/snowbound/issues/36), revision
history, multi-machine live collaboration, and much more.

**Download**: macOS: <a href="https://file.paperclover.net/shr/snowbound/latest/Snowbound-macos-aarch64.zip"><img src="docs/badges/macos-silicon.svg" alt="Silicon" align="top"></a> <a href="https://file.paperclover.net/shr/snowbound/latest/Snowbound-macos-x86_64.zip"><img src="docs/badges/macos-intel.svg" alt="Intel" align="top"></a> <a href="https://file.paperclover.net/shr/snowbound/latest/Snowbound-macos-10.6.zip"><img src="docs/badges/macos-legacy.svg" alt="OS X 10.6+" align="top"></a> • Linux: <a href="https://file.paperclover.net/shr/snowbound/latest/snowbound-linux-x86_64"><img src="docs/badges/linux-x86_64.svg" alt="x86_64" align="top"></a> <a href="https://file.paperclover.net/shr/snowbound/latest/snowbound-linux-aarch64"><img src="docs/badges/linux-aarch64.svg" alt="aarch64" align="top"></a> • Windows: <a href="https://file.paperclover.net/shr/snowbound/latest/snowbound-windows-x86_64.exe"><img src="docs/badges/windows-x64.svg" alt="x64" align="top"></a> <a href="https://file.paperclover.net/shr/snowbound/latest/snowbound-windows-aarch64.exe"><img src="docs/badges/windows-arm.svg" alt="Arm" align="top"></a>

<!-- Regenerate from the sample notebook (edit it freely in OneNote or Snowbound):
python3 tools/canvas/build_macos.py --release && d=$(mktemp -d) && cp -R docs/sample-notebook/Personal "$d" && SNOWBOUND_SCREENSHOT_SYSTEM=snow-leopard target/Snowbound.app/Contents/MacOS/Snowbound --notebook "$d/Personal" --cache "$d/cache" --settings "$d/settings.json" --screenshot "$d/screenshot" && cp "$d"/screenshot-{light,dark}.png docs/ && oxipng -o 6 --strip all docs/screenshot-{light,dark}.png
-->
<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/screenshot-dark.png">
  <img alt="Snowbound showing a garden notebook page with a to-do list, a table and a sketch on OneNote's Ivy background" src="docs/screenshot-light.png">
</picture>

Snowbound implements the file and sync protocol used in 2010 Microsoft OneNote,
so notebooks are fully compatible, including collaboration features. It supports
OS X 10.6 Snow Leopard, all the way to modern macOS, Linux, Windows
[(WIP)](https://shale.paperclover.net/snowbound/issues/11), and fully capable
iOS[(WIP)](https://shale.paperclover.net/snowbound/issues/36) and
Android[(WIP)](https://shale.paperclover.net/snowbound/issues/24) apps.

For my songwriting for *[paper clover](https://paperclover.net)*, I use this app
alongside OneNote on my Windows 7 laptop.
