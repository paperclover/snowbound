# Page date

`native` is OneNote 2010 changing a page's date (`tools/native/page-date.ps1` on
`picture-edit/native-page-level/notebook`): it creates two pages, "Kept date" and
"Changed date", each with OneNote's own title, date and time, then gives "Changed date"
the `dateTime` 2024-03-05T14:30:00Z through the COM API (`before.xml`, `update.xml`,
`after.xml`; `kept.xml` is the untouched page). In `notebook/`, the revision that change
stored sets the page metadata's `TopologyCreationTimeStamp` (`0x18001c65`), rewrites the
date and time fields' text (`TextExtendedAscii`, `0x1c003498`, in the clone's Pacific
time: "Tuesday, March 05, 2024", "7:30 AM"), and gives the two fields' elements the
change time as `CreationTimeStamp` (`0x14001d09`) and `LastModifiedTime`, with the
date outline's `LastModifiedTime`. The title, the page and the section's page metadata
keep theirs.

`candidate` is `a_page_date_stores_what_onenote_stores` in `crates/onestore/src/op/tests.rs`
(`ONESTORE_PAGE_DATE_EXPORT`): the `Date` op dates "Kept date" 2025-07-04T16:45:00Z,
showing "Friday, July 04, 2025" and "9:45 AM", and changes the same objects OneNote did.
`cold` is its fresh OneNote 2010 read: the page reads back with that `dateTime`, and
`read/page-001.png` shows the new date and time under the title.
