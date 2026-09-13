use canvas::{
    document::TextDocument,
    layout::TextEngine,
    outline::{Arrange, OutlineLayout},
};
use onestore::page::{Outline, Page, PageObject};
use onestore::{RevisionIndex, Store, document::Document};
use serde_json::json;
use std::{env, fs, io, sync::Arc, time::Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let path = args
        .next()
        .ok_or("Expected a section snapshot and exact page title")?;
    let title = args.next().ok_or("Expected an exact page title")?;
    let mut substitutes = Vec::new();
    while let Some(option) = args.next() {
        if option != "--substitute-font" {
            return Err(format!("Unknown option: {option}").into());
        }
        substitutes.push(
            args.next()
                .ok_or("Provide a font file after --substitute-font.")?,
        );
    }
    let start = Instant::now();
    let bytes = fs::read(path)?;
    let store = Store::parse(&bytes)?;
    let index = RevisionIndex::parse(&store)?;
    let document = Document::parse(&index)?;
    let page = Page::from_document(&document, &title)?;
    drop(document);
    drop(index);
    drop(store);
    drop(bytes);
    let import_ms = start.elapsed().as_secs_f64() * 1000.0;
    let start = Instant::now();
    let mut engine = TextEngine::default();
    for path in substitutes {
        engine.register_substitute(parley::fontique::Blob::new(Arc::new(fs::read(path)?)))?;
    }
    let mut objects = Vec::new();
    let mut title_areas = Vec::new();
    for object in &page.objects {
        match object {
            PageObject::Outline(outline) => objects.push(outline_json(
                outline,
                &outline.layout(&mut engine, &page.definitions)?,
                None,
            )?),
            PageObject::Title(title) => {
                title_areas.push(
                    json!({"id": title.id, "layout": title.layout, "date_outline": title.date,
                    "outlines": title.outlines.iter().map(|o| o.id).collect::<Vec<_>>()}),
                );
                for (outline, (origin, layout)) in title
                    .outlines
                    .iter()
                    .zip(title.layout(&mut engine, &page.definitions)?)
                {
                    objects.push(outline_json(outline, &layout, Some(origin))?);
                }
            }
            PageObject::Image(image) => objects.push(json!({"kind": "image", "id": image.id,
                "layout": image.layout, "bytes": image.bytes.as_ref().map(|b| b.len()),
                "alt": image.alt, "background": image.background})),
            PageObject::Ink(ink) => objects.push(json!({"kind": "ink", "id": ink.id,
                "bounds": ink.bounds(), "strokes": ink.strokes.len(), "groups": ink.groups.len()})),
            PageObject::Unsupported(object) => objects.push(json!({"kind": "unsupported",
                "id": object.id, "layout": object.layout, "jcid": object.jcid})),
        }
    }
    serde_json::to_writer_pretty(
        io::stdout().lock(),
        &json!({
            "title": page.title, "created_filetime": page.created, "title_areas": title_areas, "margin_origin": page.margin_origin, "objects": objects,
            "definitions": page.definitions.iter().map(|(id, definition)| (id.to_string(), json!({
                "kind": definition.kind, "format": definition.format,
            }))).collect::<serde_json::Map<_, _>>(), "import_ms": import_ms,
            "layout_ms": start.elapsed().as_secs_f64() * 1000.0,
            "measurement": "Outline layout applies indentation, paragraph spacing, collapsed descendants and bullet height; titles use retained parent geometry and stacked child outlines; tags and numbered markers are separate",
        }),
    )?;
    Ok(())
}

fn outline_json(
    outline: &Outline,
    laid_out: &OutlineLayout,
    title_origin: Option<[f32; 2]>,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let mut paragraphs = Vec::new();
    let document = TextDocument::from_nodes(outline.paragraphs.clone())?;
    for paragraph in document.text_nodes() {
        let placed = laid_out.paragraphs.iter().find(|p| p.id == paragraph.id);
        let source = &paragraph
            .text()
            .ok_or("Unsupported paragraph content")?
            .text;
        let projection;
        let visible = if let Some(placed) = placed {
            placed.projection.text()
        } else {
            projection = source.project()?;
            projection.text()
        };
        let layout = placed.map(|p| &p.text);
        let lines: Vec<_> = layout
            .as_ref()
            .into_iter()
            .flat_map(|layout| layout.lines())
            .map(|(line, bounds)| {
                let range = bounds.source.clone();
                json!({
                    "start_utf16": visible.utf16_offset(range.start).unwrap(),
                    "end_utf16": visible.utf16_offset(range.end).unwrap(),
                    "text": &visible.text()[range],
                    "advance": line.metrics().advance,
                    "baseline": bounds.baseline,
                    "height": bounds.height,
                })
            })
            .collect();
        paragraphs.push(json!({
                        "id": paragraph.id, "parent": paragraph.parent, "level": paragraph.level,
                        "date_fields": paragraph.text().into_iter().filter_map(|text| text.date_field).map(|field| format!("{field:?}")).collect::<Vec<_>>(),
                        "format": paragraph.format, "collapsed": paragraph.collapsed,
                        "lists": paragraph.lists,
                        "tags": paragraph.tags.iter().chain(paragraph.text().into_iter().flat_map(|t| &t.tags)).collect::<Vec<_>>(),
                        "source_utf16": source.text().encode_utf16().count(),
                        "visible_utf16": visible.text().encode_utf16().count(),
                        "visible_text": visible.text(), "spans": visible.spans().iter().map(|s| json!({"end_utf8": s.end, "format": s.format})).collect::<Vec<_>>(),
                        "height": layout.as_ref().map(|l| l.height()), "lines": lines,
                        "origin": placed.map(|p| p.origin),
                        "tag_icons": placed.map(|p| p.tags.iter().map(|tag| json!({
                            "icon": format!("{:?}", tag.icon), "label": tag.label, "disabled": tag.disabled,
                            "origin": [tag.origin[0], tag.origin[1] + p.origin[1]],
                        })).collect::<Vec<_>>()),
                        "markers": placed.map(|p| p.markers.iter().map(|(layout, origin)| json!({
                            "origin": [origin[0], origin[1] + p.origin[1]], "height": layout.height(),
                            "baseline": layout.lines().next().unwrap().1.baseline,
                            "advance": layout.lines().next().unwrap().0.metrics().advance,
                        })).collect::<Vec<_>>()),
                    }));
    }
    Ok(
        json!({"kind": "outline", "id": outline.id, "layout": outline.layout,
                    "indents": outline.indents, "is_title": title_origin.is_some(), "is_title_text": outline.title, "minimum_width": outline.min_width, "title_origin": title_origin, "paragraphs": paragraphs,
                    "size": laid_out.size,
                    "tables": laid_out.tables.iter().map(|table| json!({
                        "id": table.id, "borders": table.borders,
                        "cells": table.cells.iter().map(|cell| json!({"id": cell.id, "rect": cell.rect})).collect::<Vec<_>>()
                    })).collect::<Vec<_>>(),
                    "unsupported": outline.unsupported.iter().map(|u| u.jcid).collect::<Vec<_>>() }),
    )
}
