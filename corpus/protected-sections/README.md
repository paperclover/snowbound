# Password-protected sections

OneNote 2010 (14.0.4763.1000, Windows 7 lab) setting, changing and removing a section's
password, and Snowbound doing the same to a notebook that a fresh OneNote then cold-opens.
Passwords are fictitious and public ([manifest.json](manifest.json),
`candidate/passwords.json`).

## What OneNote does

- **Set, change, remove.** Each writes the section anew: OneNote deletes the file and
  creates one at the same path (a new NTFS file index, its creation time tunnelled), which
  grows through some 30 transactions. Every identity is new: the header's file GUID, the root
  object space, each page space (one fresh GUID for all of them, each its own `n`), and each payload's
  file-data GUID. Each space keeps its labelled revisions, each a checkpoint: the current one
  under roles 1 and 4, the version history under its context. The TOC gains an entry for
  the new identity and keeps the old one (`native/*.one` from `source/synthetic.one`).
- **The crypto.** Office Agile encryption (MS-OFFCRYPTO 2.3.4.10) inside the
  `ObjectDataEncryptionKeyV2FNDX` container (MS-ONESTORE 2.5.19): the words
  `3, length, 16, length − 16`, version `4.4` with flags `0x40`, then UTF-8 XML with a CRLF
  after the declaration and no data-integrity element. Key data and key encryptor are both
  AES-128, CBC, SHA-1, 16-byte salts; `spinCount` 100000. H0 = SHA-1(salt ‖ UTF-16LE
  password), Hn = SHA-1(LE32(n) ‖ Hn−1); each block key is SHA-1(H ‖ label)[..16] with the
  password salt as IV. The verifier hash is padded with zeros to 32 bytes. A change of
  password draws a new 16-byte content key. Property objects are their reference streams, a
  length, a random IV and CBC of `u16 padding ‖ bytes ‖ random padding`; payloads are CBC,
  IV = SHA-1(key-data salt ‖ LE32(0))[..16], of `u64 length ‖ bytes ‖ random padding`.
  Read-only declarations hash the clear bytes zero-padded to 8. Every revision OneNote
  writes anew names the key; its later dependent revisions do not.
- **Locking.** Locked, the section shows "This section is password protected. Click here or
  press ENTER to unlock this section." with no page list; the Protected Section dialog says
  "Password is incorrect." Options › Advanced › Passwords: lock after not being worked in for
  1, 5, 10 (default), 15 or 30 minutes, 1, 2, 4, 8 or 12 hours or 1 day (registry
  `Options\Save\PasswordTimeout`, minutes; it takes effect after a restart); lock on
  navigating away (off); let add-ins reach unlocked sections (on). With a minute set, a
  section idle on screen locked between 72 and 132 seconds. Ctrl+Alt+L and Lock All lock
  every section. Locking and unlocking write nothing.
- **Search and Tags Summary** leave locked sections out without saying so, and take them in
  once unlocked.
- **Other writers.** With the section locked, OneNote wrote nothing while another writer
  appended a revision, and showed it once unlocked; unlocked, it showed the next one live.
  When the other writer gave the section a new password (a new file, as above), OneNote
  showed it locked again, refused the old password and took the new one
  (`native/observed/`). Observed on a local folder with one OneNote; two OneNotes on a share
  were not tried.

`native/snowbound-changed-then-onenote-edit.one` is a section Snowbound protected, edited and
gave another password, after OneNote unlocked it and typed into it.

## Snowbound

`candidate/notebook` comes from `a_notebook_protected_through_its_sessions`
(`crates/notebook/tests/protected.rs`, `ONESTORE_PROTECTED_EXPORT`): OneNote's pages imported
into five sections, each given a password through `Notebook::set_password`; `Edited` edited
through its session three times, `Changed` given another password and edited, `Removed`
unprotected, `Conflicted` edited offline by two replicas into conflict pages.
`tools/native_protected.py` cold-opened it in a fresh clone, unlocked each section in OneNote's
dialog, typed into `Edited` through COM and read every page (`cold/`, with the notebook
OneNote left). `tools/test_protected_sections.py` checks both directions without a VM.
