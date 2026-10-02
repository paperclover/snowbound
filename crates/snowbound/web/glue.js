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

/** Writes `[path]` (removed), `[path, null]` (a folder) and `[path, length, ranges]` entries. */
export function storeFiles(changes) {
  const buffers = changes.flatMap(([, , ranges]) => (ranges ?? []).map(([, bytes]) => bytes.buffer));
  storage.postMessage({ kind: "store", changes }, buffers);
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

export function askConfirm(message) {
  return confirm(message);
}

export function askText(message, value) {
  return prompt(message, value) ?? undefined;
}

export function tell(message) {
  alert(message);
}

export function openLink(url) {
  open(url, "_blank", "noopener");
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
  input.focus({ preventScroll: true });
}
