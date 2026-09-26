//! Tables as OneNote stores them: rows and cells are plain containers, a cell carries its
//! indentation table and the flags every native cell has, the table its column widths,
//! locks and border flag.

use super::{
    Values,
    content::{NATIVE_INDENTS, measurement_bytes},
};
use crate::{
    Error, ExGuid,
    active::{ActivePage, Changes},
    page::{Table, TableColumn},
    write::PropertyObject,
};
use std::collections::{BTreeMap, BTreeSet};

fn invalid(message: &'static str) -> Error {
    Error { offset: 0, message }
}

/// Every row carries one cell per column and widths are usable.
pub(crate) fn validate_table(table: &Table) -> Result<(), Error> {
    if table.rows.is_empty() || table.columns.is_empty() {
        return Err(invalid("A table needs at least one row and one column"));
    }
    if table
        .columns
        .iter()
        .any(|c| !c.width.is_finite() || c.width < 36.0)
    {
        return Err(invalid("Table columns are at least 36 points wide"));
    }
    for row in &table.rows {
        if row.cells.len() != table.columns.len() {
            return Err(invalid("Every table row has one cell per column"));
        }
        for cell in &row.cells {
            for paragraph in &cell.paragraphs {
                if let crate::page::ParagraphContent::Table(nested) = &paragraph.content {
                    validate_table(nested)?;
                }
            }
        }
    }
    Ok(())
}

/// A table's rows and cells by stored identity, in order, and what new ones carry.
pub(crate) struct Structure<'t> {
    pub rows: Vec<(ExGuid, Vec<ExGuid>)>,
    pub new_rows: BTreeSet<ExGuid>,
    /// The indentation table and shading of each new cell.
    pub new_cells: BTreeMap<ExGuid, (&'t [f32], Option<u32>)>,
    pub columns: &'t [TableColumn],
    pub borders: Option<bool>,
}

/// The indentation table a new cell takes: its own, else the table's first cell's, else
/// the one OneNote gives new cells.
pub(crate) fn cell_indents<'t>(own: &'t [f32], template: Option<&'t [f32]>) -> &'t [f32] {
    [own, template.unwrap_or_default(), &NATIVE_INDENTS]
        .into_iter()
        .find(|indents| !indents.is_empty())
        .unwrap()
}

/// Writes table `table` as `structure`; with `holder`, creates it as that paragraph's content.
pub(crate) fn table_changes(
    active: &ActivePage<'_>,
    table: ExGuid,
    holder: Option<ExGuid>,
    structure: &Structure<'_>,
) -> Result<Changes, Error> {
    let modified = crate::create::current_timestamps()?.0.to_le_bytes();
    let mut created: Vec<(ExGuid, u32, Values)> = Vec::new();
    for (row, cells) in &structure.rows {
        for cell in cells {
            if let Some((indents, shading)) = structure.new_cells.get(cell) {
                let mut values: Values = vec![
                    (0x14001d7a, modified.to_vec()),
                    (0x0c001c13, vec![0]),
                    (0x0c001c03, vec![1]),
                    (0x88001c91, Vec::new()),
                    (0x1c001c12, measurement_bytes(indents, 4)?),
                ];
                if let Some(shading) = shading {
                    values.push((0x14001e26, shading.to_le_bytes().to_vec()));
                }
                created.push((*cell, 0x60024, values));
            }
        }
        if structure.new_rows.contains(row) {
            created.push((*row, 0x60023, vec![(0x14001d7a, modified.to_vec())]));
        }
    }
    let columns = structure.columns;
    let mut table_values: Values = vec![
        (0x14001d7a, modified.to_vec()),
        (
            0x14001d57,
            (structure.rows.len() as u32).to_le_bytes().to_vec(),
        ),
        (0x14001d58, (columns.len() as u32).to_le_bytes().to_vec()),
        (
            0x1c001d66,
            measurement_bytes(&columns.iter().map(|c| c.width).collect::<Vec<_>>(), 1)?,
        ),
    ];
    let mut locks = vec![columns.len() as u8];
    locks.extend(vec![0u8; columns.len().div_ceil(8)]);
    for (i, column) in columns.iter().enumerate() {
        if column.locked {
            locks[1 + i / 8] |= 1 << (i % 8);
        }
    }
    table_values.push((0x1c001d7d, locks));
    table_values.push((
        0x08001d5e | (u32::from(structure.borders.unwrap_or(true)) << 31),
        Vec::new(),
    ));
    if holder.is_some() {
        table_values.push((0x14001c3e, 1u32.to_le_bytes().to_vec()));
        table_values.push((0x14001c84, 1u32.to_le_bytes().to_vec()));
        created.push((table, 0x60022, Vec::new()));
    }
    let raw = &active.live.revision;
    let mut changed: Changes = BTreeMap::new();
    for (id, jcid, values) in &created {
        let mut node = PropertyObject {
            jcid: *jcid,
            bytes: crate::create::properties(values)?,
            global_ids: std::sync::Arc::new(BTreeMap::from([(0, id.guid)])),
        };
        node.reference(*id)?;
        changed.insert(*id, node);
    }
    let mut row_refs = Vec::new();
    for (row_id, cells) in &structure.rows {
        let mut row = match changed.remove(row_id) {
            Some(row) => row,
            None => PropertyObject::from_object(&raw.objects[row_id])?,
        };
        let mut cell_refs = Vec::new();
        for cell in cells {
            cell_refs.extend_from_slice(&row.reference(*cell)?);
        }
        row.set(&[(0x24001c20, &cell_refs), (0x14001d7a, &modified)])?;
        changed.insert(*row_id, row);
    }
    let mut table_object = match changed.remove(&table) {
        Some(object) => object,
        None => PropertyObject::from_object(&raw.objects[&table])?,
    };
    for (row_id, _) in &structure.rows {
        row_refs.extend_from_slice(&table_object.reference(*row_id)?);
    }
    table_object.remove(&[0x1c001d7d, 0x08001d5e])?;
    table_object.set(
        &table_values
            .iter()
            .map(|(id, bytes)| (*id, bytes.as_slice()))
            .collect::<Vec<_>>(),
    )?;
    table_object.set(&[(0x24001c20, &row_refs)])?;
    changed.insert(table, table_object);
    if let Some(holder) = holder {
        let mut object = PropertyObject::from_object(&raw.objects[&holder])?;
        let reference = object.reference(table)?;
        object.set(&[(0x24001c1f, &reference), (0x14001d7a, &modified)])?;
        changed.insert(holder, object);
    }
    Ok(changed)
}

/// Shading (the documented `CellShadingColor`, which OneNote 2010 stores but neither
/// renders nor accepts through its COM schema) and indents change in place on a cell.
pub(crate) fn cell_changes(
    active: &ActivePage<'_>,
    cell: ExGuid,
    stored: (Option<u32>, &[f32]),
    shading: Option<u32>,
    indents: &[f32],
) -> Result<Changes, Error> {
    let mut values: Values = vec![(
        0x14001d7a,
        crate::create::current_timestamps()?
            .0
            .to_le_bytes()
            .to_vec(),
    )];
    let mut removed = Vec::new();
    if shading != stored.0 {
        match shading {
            Some(shading) => values.push((0x14001e26, shading.to_le_bytes().to_vec())),
            None => removed.push(0x14001e26),
        }
    }
    if indents != stored.1 {
        if indents.is_empty() {
            return Err(invalid("A table cell needs an indentation table"));
        }
        values.push((0x1c001c12, measurement_bytes(indents, 4)?));
    }
    let raw = &active.live.revision;
    let mut object = PropertyObject::from_object(&raw.objects[&cell])?;
    object.remove(&removed)?;
    let values: Vec<(u32, &[u8])> = values
        .iter()
        .map(|(id, bytes)| (*id, bytes.as_slice()))
        .collect();
    object.set(&values)?;
    Ok(BTreeMap::from([(cell, object)]))
}
