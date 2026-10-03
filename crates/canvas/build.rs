//! Deflates the font clones the target embeds into `bundled.rs`, a table `layout.rs` includes,
//! leaving out each whose family the target's system ships.

/// Metric-compatible clones, by the family each stands in for.
const CLONES: [(&str, &[&str]); 4] = [
    (
        "Calibri",
        &[
            "Carlito-Regular",
            "Carlito-Bold",
            "Carlito-Italic",
            "Carlito-BoldItalic",
        ],
    ),
    ("Arial", &["Arimo", "Arimo-Italic"]),
    (
        "Times New Roman",
        &[
            "Tinos-Regular",
            "Tinos-Bold",
            "Tinos-Italic",
            "Tinos-BoldItalic",
        ],
    ),
    (
        "Courier New",
        &[
            "Cousine-Regular",
            "Cousine-Bold",
            "Cousine-Italic",
            "Cousine-BoldItalic",
        ],
    ),
];

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=assets/fonts");
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();
    let browser = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap() == "wasm32";
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let mut table = String::from("&[\n");
    for (family, faces) in CLONES {
        // The browser fetches them beside the module instead.
        let shipped = browser
            || match os.as_str() {
                "windows" => true,
                "macos" | "ios" => family != "Calibri",
                _ => false,
            };
        if shipped {
            continue;
        }
        table += &format!("    ({family:?}, &[\n");
        for face in faces {
            let font = std::fs::read(format!("assets/fonts/{face}.ttf")).unwrap();
            let deflated = format!("{face}.ttf.deflate");
            std::fs::write(
                out.join(&deflated),
                miniz_oxide::deflate::compress_to_vec(&font, 10),
            )
            .unwrap();
            // Relative to OUT_DIR, so a target folder reached by another path still finds them.
            table +=
                &format!("        include_bytes!(concat!(env!(\"OUT_DIR\"), \"/{deflated}\")),\n");
        }
        table += "    ]),\n";
    }
    table += "]\n";
    std::fs::write(out.join("bundled.rs"), table).unwrap();
}
