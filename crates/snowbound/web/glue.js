// The browser's half of Snowbound (src/web.rs): the page's input, its dialogs and files, and
// the files `notebook::fs` holds, which a storage worker keeps in the origin's private file
// system (OPFS) between visits.

let wasm;
let canvas;
let input;
let picker;
let framePending = false;
let wakeTimer;
let storage;

/** The storage worker: the only place OPFS hands out synchronous handles, which write a
 * file's changed ranges in place. Runs as a worker of its own, from this source. */
function storageWorker() {
  // IndexedDB held the files before OPFS did: `files` by path, and before that the first web
  // build's `sections`, which move into a notebook of their own.
  const DB = "snowbound";
  const MIGRATED = "/Notebooks/Web Notebook";
  const handles = new Map();
  let root;
  let queue = Promise.resolve();

  const names = (path) => path.split("/").filter(Boolean);
  const folder = async (parts) => {
    let dir = root;
    for (const name of parts) dir = await dir.getDirectoryHandle(name, { create: true });
    return dir;
  };
  const handle = async (path) => {
    if (!handles.has(path)) {
      const parts = names(path);
      const file = await (await folder(parts.slice(0, -1))).getFileHandle(parts.at(-1), { create: true });
      handles.set(path, await file.createSyncAccessHandle());
    }
    return handles.get(path);
  };
  const close = (path) => {
    for (const [held, open] of handles)
      if (held === path || held.startsWith(`${path}/`)) {
        open.close();
        handles.delete(held);
      }
  };
  const write = async (path, length, ranges) => {
    const open = await handle(path);
    for (const [offset, bytes] of ranges) open.write(bytes, { at: offset });
    open.truncate(length);
    open.flush();
  };
  const remove = async (path) => {
    close(path);
    const parts = names(path);
    try {
      await (await folder(parts.slice(0, -1))).removeEntry(parts.at(-1), { recursive: true });
    } catch (error) {
      if (error.name !== "NotFoundError") throw error;
    }
  };
  const list = async (dir, path, out) => {
    for await (const [name, entry] of dir.entries()) {
      const at = `${path}/${name}`;
      if (entry.kind === "directory") {
        out.push([at, null, 0]);
        await list(entry, at, out);
      } else {
        const file = await entry.getFile();
        out.push([at, new Uint8Array(await file.arrayBuffer()), file.lastModified]);
      }
    }
    return out;
  };
  const migrate = async () => {
    const db = await new Promise((resolve, reject) => {
      const open = indexedDB.open(DB);
      open.onsuccess = () => resolve(open.result);
      open.onerror = () => reject(open.error);
    });
    const stores = [...db.objectStoreNames];
    const all = (store) =>
      new Promise((resolve, reject) => {
        const request = db.transaction(store).objectStore(store).getAll();
        request.onsuccess = () => resolve(request.result);
        request.onerror = () => reject(request.error);
      });
    const files = stores.includes("files") ? await all("files") : [];
    const sections = stores.includes("sections") ? await all("sections") : [];
    db.close();
    const moved = files.length
      ? files
      : sections
          .sort((a, b) => a.order - b.order)
          .map(({ file, bytes }) => ({ path: `${MIGRATED}/${file}`, bytes }));
    for (const { path, bytes } of moved)
      if (bytes) await write(path, bytes.length, [[0, bytes]]);
      else await folder(names(path));
    await new Promise((resolve) => {
      const removal = indexedDB.deleteDatabase(DB);
      removal.onsuccess = removal.onerror = removal.onblocked = resolve;
    });
  };

  onmessage = ({ data }) => {
    queue = queue
      .then(async () => {
        if (data.kind === "load") {
          root = await navigator.storage.getDirectory();
          let files = await list(root, "", []);
          if (!files.length) {
            await migrate();
            files = await list(root, "", []);
          }
          postMessage(files, files.flatMap(([, bytes]) => (bytes ? [bytes.buffer] : [])));
        } else
          for (const [path, length, ranges] of data.changes)
            if (length === undefined) await remove(path);
            else if (length === null) await folder(names(path));
            else await write(path, length, ranges);
      })
      .catch((error) => console.error("Keeping files", error));
  };
}

/** Every file kept, as `[path, bytes or null for a folder, modified]`. */
export function loadFiles() {
  const source = URL.createObjectURL(new Blob([`(${storageWorker})()`], { type: "text/javascript" }));
  storage = new Worker(source);
  storage.postMessage({ kind: "load" });
  return new Promise((resolve, reject) => {
    storage.onmessage = ({ data }) => resolve(data);
    storage.onerror = (error) => reject(new Error(`The storage worker failed: ${error.message}`));
  });
}

/** Writes `[path]` (removed), `[path, null]` (a folder) and `[path, length, ranges]` entries;
 * those under a folder of the user's go there, a committed section only where nothing else
 * wrote it since it was read. */
export function storeFiles(changes) {
  const kept = changes.filter(([path]) => !folderOf(path));
  for (const change of changes) if (folderOf(change[0])) serially(() => writeFolder(change));
  if (kept.length)
    storage.postMessage(
      { kind: "store", changes: kept },
      kept.flatMap(([, , ranges]) => (ranges ?? []).map(([, bytes]) => bytes.buffer)),
    );
}

// Folders of the user's, through the File System Access API (Chromium): mirrored in
// `notebook::fs` under FOLDERS, their handles kept in IndexedDB to reach them again.
const FOLDERS = "/Folders";
const folders = new Map();
let writing = Promise.resolve();
const serially = (work) =>
  (writing = writing.then(work).catch((error) => console.error("Writing to the folder", error)));
const folderOf = (path) =>
  [...folders.keys()].find((root) => path === root || path.startsWith(`${root}/`));
const stamp = (file) => `${file.size}:${file.lastModified}`;

function kept(mode, act) {
  return new Promise((resolve, reject) => {
    const open = indexedDB.open("snowbound-folders", 1);
    open.onupgradeneeded = () => open.result.createObjectStore("folders", { keyPath: "root" });
    open.onerror = () => reject(open.error);
    open.onsuccess = () => {
      const request = act(open.result.transaction("folders", mode).objectStore("folders"));
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error);
    };
  });
}

/** Every file and folder below `dir`, at `path`: `[path, File or null]`. */
async function walk(dir, path, out = []) {
  for await (const [name, entry] of dir.entries()) {
    const at = `${path}/${name}`;
    if (entry.kind === "directory") {
      out.push([at, null]);
      await walk(entry, at, out);
    } else out.push([at, await entry.getFile()]);
  }
  return out;
}

/** Mirrors the folder `handle` at `root`: its files as `loadFiles` gives them. */
async function mirror(root, handle) {
  const known = new Map();
  folders.set(root, { handle, known });
  const files = [
    [FOLDERS, null, 0],
    [root, null, 0],
  ];
  for (const [path, file] of await walk(handle, root)) {
    known.set(path, file ? stamp(file) : "folder");
    files.push(file ? [path, new Uint8Array(await file.arrayBuffer()), file.lastModified] : [path, null, 0]);
  }
  return files;
}

/** The folders given before that the browser may still reach: `[root, files]`. Each other
 * folder offers to reconnect, which needs a click. */
export async function loadFolders() {
  const out = [];
  for (const { root, handle } of await kept("readonly", (store) => store.getAll())) {
    if ((await handle.queryPermission({ mode: "readwrite" })) === "granted")
      out.push([root, await mirror(root, handle)]);
    else {
      const button = Object.assign(document.createElement("button"), {
        className: "reconnect",
        textContent: `Reconnect “${handle.name}”`,
      });
      button.onclick = async () => {
        if ((await handle.requestPermission({ mode: "readwrite" })) === "granted") location.reload();
      };
      document.body.append(button);
    }
  }
  // Other apps' writes reach the page once they land, as a watch would report them.
  setInterval(() => serially(look), 3000);
  return out;
}

/** Hands the page what changed in each folder since it was last read. */
async function look() {
  for (const [root, folder] of folders) {
    const seen = new Map();
    for (const [path, file] of await walk(folder.handle, root)) {
      seen.set(path, file ? stamp(file) : "folder");
      if (folder.known.get(path) !== seen.get(path))
        wasm.refreshed(path, file ? new Uint8Array(await file.arrayBuffer()) : null, file?.lastModified ?? 0);
    }
    for (const path of folder.known.keys()) if (!seen.has(path)) wasm.refreshed(path, undefined, 0);
    folder.known = seen;
  }
}

async function locate(path) {
  const root = folderOf(path);
  const parts = path.slice(root.length).split("/").filter(Boolean);
  let dir = folders.get(root).handle;
  for (const name of parts.slice(0, -1)) dir = await dir.getDirectoryHandle(name, { create: true });
  return [dir, parts.at(-1), folders.get(root)];
}

async function writeFolder([path, length, ranges, committed]) {
  if (folders.has(path)) return;
  const [dir, name, folder] = await locate(path);
  if (length === undefined) {
    await dir.removeEntry(name, { recursive: true }).catch(() => {});
    for (const known of [...folder.known.keys()])
      if (known === path || known.startsWith(`${path}/`)) folder.known.delete(known);
    return;
  }
  if (length === null) {
    await dir.getDirectoryHandle(name, { create: true });
    folder.known.set(path, "folder");
    return;
  }
  const handle = await dir.getFileHandle(name, { create: true });
  const before = await handle.getFile();
  if (committed && folder.known.get(path) !== stamp(before)) {
    // Another app wrote the section since it was read: the commit stands on what it found
    // no longer, so the page takes the file as it is and its edits go again on top.
    folder.known.set(path, stamp(before));
    return wasm.refreshed(path, new Uint8Array(await before.arrayBuffer()), before.lastModified);
  }
  const writable = await handle.createWritable({ keepExistingData: !committed });
  for (const [offset, bytes] of ranges) await writable.write({ type: "write", position: offset, data: bytes });
  await writable.truncate(length);
  await writable.close();
  const after = await handle.getFile();
  folder.known.set(path, stamp(after));
  if (committed) wasm.refreshed(path, ranges[0][1], after.lastModified);
}

/** Asks for a notebook: its folder where the browser can be given one, else files to copy in. */
export function pickNotebook() {
  if (!window.showDirectoryPicker) return pickFiles("open", ".one,.onetoc2,.onepkg");
  showDirectoryPicker({ id: "notebook", mode: "readwrite" })
    .then(async (handle) => {
      for (const [root, folder] of folders)
        if (await folder.handle.isSameEntry(handle)) return wasm.mounted(root, []);
      let root = `${FOLDERS}/${handle.name}`;
      for (let number = 2; folders.has(root); number++) root = `${FOLDERS}/${handle.name} ${number}`;
      await kept("readwrite", (store) => store.put({ root, handle }));
      wasm.mounted(root, await mirror(root, handle));
    })
    .catch((error) => error.name === "AbortError" || console.error("Opening the folder", error));
}

export function fetchFont(name) {
  fetch(`fonts/${name}`)
    .then((response) => (response.ok ? response.arrayBuffer() : Promise.reject(response.status)))
    .then((data) => wasm.font_arrived(new Uint8Array(data)))
    .catch((error) => console.warn("Font", name, error));
}

export function requestFrame() {
  if (!framePending) {
    framePending = true;
    requestAnimationFrame(() => {
      framePending = false;
      wasm.frame();
    });
  }
}

export function wakeIn(ms) {
  clearTimeout(wakeTimer);
  wakeTimer = setTimeout(requestFrame, Math.max(0, ms));
}

export function setCursor(name) {
  if (canvas.style.cursor !== name) canvas.style.cursor = name;
}

export function setTitle(title) {
  document.title = title;
}

/** Puts the text area at the caret, so an input method's candidates show beside it. */
export function placeInput(x, y, height, text) {
  input.style.left = `${x}px`;
  input.style.top = `${y}px`;
  input.style.height = `${height}px`;
  input.readOnly = !text;
}

export function writeClipboard(text) {
  navigator.clipboard.writeText(text).catch((error) => console.warn("Copy", error));
}

export function download(name, bytes, kind) {
  const url = URL.createObjectURL(new Blob([bytes], { type: kind }));
  const link = Object.assign(document.createElement("a"), { href: url, download: name });
  link.click();
  setTimeout(() => URL.revokeObjectURL(url), 10_000);
}

/** Asks for files for `purpose` (`open` or `place`), of the extensions `accept` lists. */
export function pickFiles(purpose, accept) {
  picker.dataset.purpose = purpose;
  picker.accept = accept;
  picker.multiple = purpose === "open";
  picker.click();
}

/** The long date and short time a new page's title shows, as OneNote dates one. */
export function dateText(ms) {
  const date = new Date(ms);
  return [
    date.toLocaleDateString(undefined, {
      weekday: "long",
      year: "numeric",
      month: "long",
      day: "numeric",
    }),
    date.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" }),
  ];
}

export function shortDate(ms) {
  return new Date(ms).toLocaleDateString();
}

/** Shows `message` and `detail` in a dialog over the page, with a field holding `value`
 * unless that is undefined, and `buttons`, the first the default. Resolves to the index of
 * the button pressed, -1 when dismissed, and the field's text. */
function ask(message, detail, buttons, value) {
  const dialog = Object.assign(document.createElement("dialog"), { className: "ask" });
  const form = Object.assign(document.createElement("form"), { method: "dialog" });
  const field = value === undefined ? null : Object.assign(document.createElement("input"), { value });
  const row = document.createElement("div");
  row.append(...buttons.map((label, index) =>
    Object.assign(document.createElement("button"), { value: index, textContent: label })));
  form.append(...[field, row].filter(Boolean));
  dialog.append(Object.assign(document.createElement("h1"), { textContent: message }));
  if (detail) dialog.append(Object.assign(document.createElement("p"), { textContent: detail }));
  dialog.append(form);
  document.body.append(dialog);
  dialog.showModal();
  return new Promise((resolve) =>
    dialog.addEventListener("close", () => {
      dialog.remove();
      resolve([dialog.returnValue === "" ? -1 : Number(dialog.returnValue), field?.value]);
    }),
  );
}

export function askConfirm(message, detail, cancel, action) {
  return ask(message, detail, [action, cancel]).then(([pressed]) => pressed === 0);
}

export function askText(message, value) {
  return ask(message, "", ["OK", "Cancel"], value).then(([pressed, text]) =>
    pressed === 0 ? text : undefined,
  );
}

export function tell(message, detail) {
  ask(message, detail, ["OK"]);
}

export function openLink(url) {
  open(url, "_blank", "noopener");
}

// Assistive technology's view of the window: AccessKit's trees mirrored as elements with
// ARIA roles, visually hidden, which a screen reader reads and acts on.
const ROLES = {
  Button: "button", DefaultButton: "button", CheckBox: "checkbox", RadioButton: "radio",
  Switch: "switch", TextInput: "textbox", MultilineTextInput: "textbox", SearchInput: "searchbox",
  Document: "document", Group: "group", GenericContainer: "generic", Label: "none",
  Paragraph: "paragraph", List: "list", ListItem: "listitem", ListBox: "listbox",
  ListBoxOption: "option", Menu: "menu", MenuBar: "menubar", MenuItem: "menuitem",
  MenuItemCheckBox: "menuitemcheckbox", MenuItemRadio: "menuitemradio", Tab: "tab",
  TabList: "tablist", TabPanel: "tabpanel", Toolbar: "toolbar", Tooltip: "tooltip",
  Tree: "tree", TreeItem: "treeitem", Dialog: "dialog", AlertDialog: "alertdialog",
  Image: "img", Link: "link", Heading: "heading", Table: "table", Row: "row", Cell: "cell",
  ColumnHeader: "columnheader", ComboBox: "combobox", ScrollBar: "scrollbar", Slider: "slider",
  Window: "application", Pane: "region", Separator: "separator", Status: "status",
};
const mirrored = new Map();
export function mirrorTree({ tree, focus, root, nodes }) {
  if (!mirrored.has(tree)) mirrored.set(tree, { nodes: new Map(), root: null });
  const held = mirrored.get(tree);
  const element = (id) => {
    if (!held.nodes.has(id)) {
      const made = document.createElement("div");
      made.id = `a11y-${tree}-${id}`;
      made.onclick = () => wasm.access(tree, id, 0);
      held.nodes.set(id, made);
    }
    return held.nodes.get(id);
  };
  for (const [id, role, name, value, children, , grafted, disabled, toggled] of nodes) {
    const node = element(id);
    const aria = ROLES[role];
    if (aria && aria !== "none") node.setAttribute("role", aria);
    else node.removeAttribute("role");
    const text = role === "Label" || role === "TextRun" || role === "StaticText";
    if (name && !text) node.setAttribute("aria-label", name);
    else node.removeAttribute("aria-label");
    node.toggleAttribute("aria-disabled", disabled);
    if (toggled === null) node.removeAttribute("aria-checked");
    else node.setAttribute("aria-checked", String(toggled));
    const kids = children.map(element);
    if (grafted && mirrored.get(grafted)?.root) kids.push(mirrored.get(grafted).root);
    if (text && !kids.length) node.textContent = value ?? name ?? "";
    else node.replaceChildren(...kids);
    if (grafted) node.dataset.grafted = grafted;
  }
  if (root !== null) {
    held.root = element(root);
    if (tree === "00000000-0000-0000-0000-000000000000")
      document.getElementById("a11y").replaceChildren(held.root);
    else
      for (const { nodes: all } of mirrored.values())
        for (const node of all.values()) if (node.dataset.grafted === tree) node.append(held.root);
  }
  if (held.nodes.has(focus)) input.setAttribute("aria-activedescendant", held.nodes.get(focus).id);
}

/** Turns the mirror on: once a screen reader asks for it, and on every visit after. */
function offerAccessibility() {
  const on = () => {
    localStorage.setItem("snowbound-accessibility", "on");
    document.getElementById("accessible").remove();
    wasm.accessibility(true);
  };
  document.getElementById("accessible").onclick = on;
  if (localStorage.getItem("snowbound-accessibility") === "on") on();
}

function modifiers(event) {
  return (
    (event.shiftKey ? 1 : 0) |
    (event.ctrlKey ? 2 : 0) |
    (event.altKey ? 4 : 0) |
    (event.metaKey ? 8 : 0)
  );
}

function read(list) {
  return Promise.all(
    [...list].map(async (file) => [file.name, new Uint8Array(await file.arrayBuffer())]),
  );
}

const NOTEBOOK_FILES = /\.(one|onetoc2|onepkg)$/i;

/** Wires the page's canvas, text area and file input to `module`'s exports. */
export function attach(module) {
  wasm = module;
  canvas = document.getElementById("page");
  input = document.getElementById("input");
  picker = document.getElementById("files");
  const point = (event) => {
    const rect = canvas.getBoundingClientRect();
    return [event.clientX - rect.left, event.clientY - rect.top];
  };
  const pressure = (event) => (event.pointerType === "pen" ? event.pressure : NaN);

  // A finger pans the page or, with a second, pinches it; one that doesn't move taps it.
  const touches = new Map();
  let gesture = null;
  const touchMoved = (event) => {
    const last = touches.get(event.pointerId);
    const now = point(event);
    touches.set(event.pointerId, now);
    if (touches.size === 1) {
      if (!gesture.panning && Math.hypot(now[0] - gesture.start[0], now[1] - gesture.start[1]) > 8)
        gesture.panning = true;
      if (gesture.panning) {
        wasm.pointer(0, ...now, -1, NaN, 0);
        wasm.wheel(now[0] - last[0], now[1] - last[1]);
      }
    } else if (touches.size === 2) {
      const [a, b] = [...touches.values()];
      const distance = Math.hypot(a[0] - b[0], a[1] - b[1]);
      const centre = [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2];
      if (gesture.distance) {
        wasm.pinch(distance / gesture.distance, centre[0], centre[1]);
        wasm.wheel(centre[0] - gesture.centre[0], centre[1] - gesture.centre[1]);
      }
      Object.assign(gesture, { distance, centre, panning: true });
    }
  };

  canvas.addEventListener("pointerdown", (event) => {
    event.preventDefault();
    input.focus({ preventScroll: true });
    canvas.setPointerCapture(event.pointerId);
    if (event.pointerType === "touch") {
      touches.set(event.pointerId, point(event));
      if (touches.size === 1) gesture = { start: point(event), panning: false };
      else gesture.distance = 0;
      return;
    }
    wasm.pointer(0, ...point(event), -1, pressure(event), modifiers(event));
    wasm.pointer(1, ...point(event), event.button, pressure(event), modifiers(event));
  });
  canvas.addEventListener("pointermove", (event) => {
    if (event.pointerType === "touch") {
      if (touches.has(event.pointerId)) touchMoved(event);
      return;
    }
    for (const each of event.getCoalescedEvents?.() ?? [event])
      wasm.pointer(0, ...point(each), -1, pressure(each), modifiers(event));
  });
  const up = (event) => {
    if (event.pointerType === "touch") {
      if (!touches.delete(event.pointerId)) return;
      if (touches.size === 0 && !gesture.panning && event.type === "pointerup") {
        wasm.pointer(0, ...point(event), -1, NaN, 0);
        wasm.pointer(1, ...point(event), 0, NaN, 0);
        wasm.pointer(2, ...point(event), 0, NaN, 0);
        wasm.pointer(3, 0, 0, -1, NaN, 0);
      }
      return;
    }
    wasm.pointer(2, ...point(event), event.button, pressure(event), modifiers(event));
  };
  canvas.addEventListener("pointerup", up);
  canvas.addEventListener("pointercancel", up);
  canvas.addEventListener("pointerleave", (event) => {
    if (event.pointerType !== "touch") wasm.pointer(3, 0, 0, -1, NaN, modifiers(event));
  });
  canvas.addEventListener("contextmenu", (event) => event.preventDefault());
  canvas.addEventListener(
    "wheel",
    (event) => {
      event.preventDefault();
      const unit = [1, 32, canvas.clientHeight][event.deltaMode];
      // The page scrolls whatever lies under the pointer, which a wheel alone doesn't move.
      wasm.pointer(0, ...point(event), -1, NaN, modifiers(event));
      // Chrome and Safari report a trackpad pinch as a wheel with Control held.
      if (event.ctrlKey) wasm.pinch(Math.exp(-event.deltaY * unit * 0.005), ...point(event));
      else wasm.wheel(-event.deltaX * unit, -event.deltaY * unit);
    },
    { passive: false },
  );

  input.addEventListener("keydown", (event) => {
    if (event.isComposing || ["Process", "Dead", "Unidentified"].includes(event.key)) return;
    const bits = modifiers(event);
    if (!wasm.takes(event.key, bits)) return;
    event.preventDefault();
    const shortcut = event.ctrlKey || event.metaKey;
    const text = [...event.key].length === 1 && !shortcut ? event.key : undefined;
    wasm.key(event.key, text, bits);
  });
  input.addEventListener("keyup", (event) => wasm.modifiers(modifiers(event)));
  input.addEventListener("compositionupdate", (event) => wasm.compose(event.data));
  input.addEventListener("compositionend", (event) => {
    wasm.commit(event.data);
    input.value = "";
  });
  // Text arriving outside keys and composition, as from a phone's keyboard or dictation.
  input.addEventListener("input", (event) => {
    if (event.isComposing) return;
    if (event.inputType === "insertText" && event.data) wasm.commit(event.data);
    input.value = "";
  });
  input.addEventListener("paste", async (event) => {
    event.preventDefault();
    const data = event.clipboardData;
    const text = data.getData("text/plain") || undefined;
    const html = data.getData("text/html") || undefined;
    wasm.paste(text, html, await read(data.files));
  });
  input.addEventListener("focus", () => wasm.focus(true));
  input.addEventListener("blur", () => wasm.focus(false));

  picker.addEventListener("change", async () => {
    const files = [...picker.files];
    const purpose = picker.dataset.purpose ?? (files.every((file) => NOTEBOOK_FILES.test(file.name)) ? "open" : "place");
    wasm.files(purpose, await read(files));
    picker.value = "";
  });
  document.addEventListener("dragover", (event) => event.preventDefault());
  document.addEventListener("drop", async (event) => {
    event.preventDefault();
    const dropped = [...event.dataTransfer.files];
    const notebooks = dropped.filter((file) => NOTEBOOK_FILES.test(file.name));
    const others = dropped.filter((file) => !NOTEBOOK_FILES.test(file.name));
    if (notebooks.length) wasm.files("open", await read(notebooks));
    if (others.length) wasm.files("place", await read(others));
  });
  addEventListener("pagehide", () => wasm.flush());
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "hidden") wasm.flush();
  });
  matchMedia("(prefers-color-scheme: dark)").addEventListener("change", (query) =>
    wasm.appearance_changed(query.matches),
  );

  const resized = (width, height) => {
    canvas.width = width;
    canvas.height = height;
    wasm.resize(width, height, devicePixelRatio);
  };
  // Device pixels from the CSS size: emulated pixel ratios leave devicePixelContentBoxSize
  // at the CSS size.
  new ResizeObserver(([entry]) =>
    resized(
      Math.round(entry.contentRect.width * devicePixelRatio),
      Math.round(entry.contentRect.height * devicePixelRatio),
    ),
  ).observe(canvas);
  offerAccessibility();
  input.focus({ preventScroll: true });
}
