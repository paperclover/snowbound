# App icon

`Assets.car` is `sources/Snowbound-Tahoe.icon` compiled by Xcode 26.0.1's `actool`,
which can leave out the flattened renditions later versions always add. macOS 26 draws
the Liquid Glass icon from it; earlier versions find no rendition there and draw
`Snowbound-Sequoia.icns` (`tools/canvas/build_macos.py` bundles both). After changing
the `.icon`, regenerate it with Xcode 26.0.1 (not newer), for the Intel floor:

```sh
DEVELOPER_DIR=/Applications/Xcode-26.0.1.app/Contents/Developer xcrun actool \
  sources/Snowbound-Tahoe.icon --compile OUT --app-icon Snowbound-Tahoe \
  --enable-on-demand-resources NO --development-region en --target-device mac \
  --platform macosx --enable-icon-stack-fallback-generation=disabled \
  --include-all-app-icons --minimum-deployment-target 10.13 \
  --output-partial-info-plist OUT/partial.plist
cp OUT/Assets.car Assets.car
```

It warns "Failed to generate flattened icon stack", which is the point.
`xcrun assetutil --info Assets.car` should list no `Icon Image` renditions.
