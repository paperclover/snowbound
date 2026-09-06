use canvas::{layout::TextEngine, text::Paragraph};
use onestore::document::Format;
use serde::{Deserialize, Serialize};
use serde_json::json;
use skrifa::{FontRef, MetadataProvider, raw::TableProvider, string::StringId};
use std::{env, fs, io, sync::Arc};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    width: f32,
    runs: Vec<Run>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Run {
    text: String,
    font: String,
    size: f32,
    bold: bool,
    italic: bool,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args_os().skip(1);
    let path = args.next().ok_or("Expected a text-case JSON file")?;
    let cases: Vec<Case> = serde_json::from_slice(&fs::read(path)?)?;
    let mut engine = TextEngine::default();
    while let Some(path) = args.next() {
        if path == "--substitute-font" {
            let path = args
                .next()
                .ok_or("Provide a font file after --substitute-font.")?;
            engine.register_substitute(parley::fontique::Blob::new(Arc::new(fs::read(path)?)))?;
            continue;
        }
        let data = parley::fontique::Blob::new(Arc::new(fs::read(path)?));
        if engine
            .fonts
            .collection
            .register_fonts(data, None)
            .is_empty()
        {
            return Err("The supplied file contains no supported fonts".into());
        }
    }
    let mut results = Vec::new();
    for case in cases {
        if !case.width.is_finite() || case.width <= 0.0 || case.runs.is_empty() {
            return Err(
                format!("{}: expected positive width and at least one run", case.id).into(),
            );
        }
        let text: String = case.runs.iter().map(|r| r.text.as_str()).collect();
        let paragraph = Paragraph::from_runs(case.runs.iter().map(|run| {
            (
                run.text.clone(),
                Format {
                    font: Some(run.font.clone()),
                    font_size: Some(run.size),
                    bold: Some(run.bold),
                    italic: Some(run.italic),
                    ..Format::default()
                },
            )
        }));
        let layout = engine
            .layout(&paragraph, case.width)
            .map_err(|error| format!("{}: {error}", case.id))?;
        let lines: Vec<_> = layout.lines().map(|(line, native)| {
            let range = native.source.clone();
            let metrics = line.metrics();
            let runs: Vec<_> = line.runs().map(|run| {
                let data = &run.font().font;
                let font = FontRef::from_index(data.data.as_ref(), data.index)
                    .expect("Parley returned an unreadable font");
                let os2 = font.os2().ok();
                json!({
                    "face": font.localized_strings(StringId::POSTSCRIPT_NAME).english_or_first().map(|s| s.to_string()),
                    "version": font.localized_strings(StringId::VERSION_STRING).english_or_first().map(|s| s.to_string()),
                    "size": run.font_size(),
                    "normalized_variation_coords": run.normalized_coords().iter().map(|c| c.to_bits()).collect::<Vec<_>>(),
                    "units_per_em": font.head().ok().map(|h| h.units_per_em()),
                    "win_ascent": os2.as_ref().map(|o| o.us_win_ascent()),
                    "win_descent": os2.as_ref().map(|o| o.us_win_descent()),
                })
            }).collect();
            json!({
                "start_utf16": text[..range.start].encode_utf16().count(),
                "end_utf16": text[..range.end].encode_utf16().count(),
                "text": &text[range],
                "advance": metrics.advance,
                "trailing_whitespace": metrics.trailing_whitespace,
                "baseline": metrics.baseline,
                "height": metrics.line_height,
                "windows_baseline": native.baseline,
                "windows_height": native.height,
                "runs": runs,
            })
        }).collect();
        results.push(
            json!({"id": case.id, "width": case.width, "requested_runs": case.runs,
                            "height": lines.iter().map(|l| l["height"].as_f64().unwrap()).sum::<f64>(),
                            "windows_height": layout.height(), "lines": lines}),
        );
    }
    serde_json::to_writer_pretty(
        io::stdout().lock(),
        &json!({
            "engine": "parley", "units": "points", "cases": results,
        }),
    )?;
    Ok(())
}
