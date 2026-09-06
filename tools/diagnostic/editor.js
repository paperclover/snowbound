const route = location.pathname.match(/^\/g\/(\d+)\/report\/(page-\d+\.html)$/);
const bar = document.createElement('aside');
bar.className = 'diagnostic';
bar.innerHTML = `<strong>Notebook copy</strong>
  <button id="edit-mode" type="button" aria-pressed="false">Edit text</button>
  <label>Text action <select id="text-action"><option value="text">Replace</option><option value="format">Format</option></select></label>
  <button id="add-paragraph" type="button">Add paragraph</button>
  <button id="add-outline" type="button">Add outline</button>
  <a href="/latest">Latest notebook</a>
  <span id="editor-status" role="status"></span>`;
document.querySelector('main').prepend(bar);
const status = document.querySelector('#editor-status');
const mode = document.querySelector('#edit-mode');
for (const control of bar.querySelectorAll('button, select')) control.disabled = !route;
const dialog = document.createElement('dialog');
dialog.setAttribute('aria-labelledby', 'edit-heading');
dialog.innerHTML = `<form><h2 id="edit-heading">Edit text</h2>
  <p id="edit-hint"></p>
  <fieldset id="placement"><legend>Paragraph placement</legend>
    <label>Parent container <select id="parent"></select></label>
    <label>Insert before <select id="before"></select></label>
  </fieldset>
  <fieldset id="coordinates"><legend>Outline position</legend>
    <label>X (points) <input id="outline-x" type="number" step="any" value="144"></label>
    <label>Y (points) <input id="outline-y" type="number" step="any" value="144"></label>
  </fieldset>
  <label for="replacement">Text</label><textarea id="replacement" rows="6"></textarea>
  <label id="author-label">Author <input id="author" value="Diagnostic"></label>
  <fieldset id="formatting"><legend>Character formatting</legend>
    <p id="format-range" role="status"></p>
    <div class="format-grid"></div>
    <label>Font <input id="font" placeholder="Keep current font"></label>
    <label>Size (points) <input id="font-size" type="number" min="6" max="130" step="0.5" placeholder="Keep current size"></label>
    <label for="color-mode">Text color</label><select id="color-mode"><option value="keep">Keep</option><option value="clear">Automatic</option><option value="set">Set color</option></select><input id="color" type="color" value="#123456" aria-label="Text color value">
    <label for="highlight-mode">Highlight</label><select id="highlight-mode"><option value="keep">Keep</option><option value="clear">Clear</option><option value="set">Set color</option></select><input id="highlight" type="color" value="#ffff00" aria-label="Highlight color value">
  </fieldset>
  <p id="save-status" role="status"></p>
  <a id="inspect-latest" target="_blank" rel="noopener" hidden>Open latest page</a>
  <div class="edit-actions"><button type="button" id="cancel-edit">Cancel</button><button id="save-edit">Save</button></div>
  </form>`;
document.body.append(dialog);
for (const name of ['Bold', 'Italic', 'Underline', 'Strike', 'Superscript', 'Subscript']) {
  const label = document.createElement('label');
  label.textContent = name === 'Strike' ? 'Strikethrough' : name;
  const select = document.createElement('select');
  select.dataset.attribute = name;
  for (const [value, text] of [['', 'Keep'], ['true', 'On'], ['false', 'Off']]) select.add(new Option(text, value));
  label.append(select);
  dialog.querySelector('.format-grid').append(label);
}
const draft = document.querySelector('#replacement');
const save = document.querySelector('#save-edit');
const cancel = document.querySelector('#cancel-edit');
const message = document.querySelector('#save-status');
const inspect = document.querySelector('#inspect-latest');
const parent = document.querySelector('#parent');
const before = document.querySelector('#before');
let selected = null;
let opening = false;
let submitting = false;
let initial = '';

mode.addEventListener('click', () => {
  const editing = document.body.classList.toggle('editing');
  mode.setAttribute('aria-pressed', String(editing));
  mode.textContent = editing ? 'Stop editing' : 'Edit text';
  for (const element of document.querySelectorAll('[data-text-object]')) {
    if (editing) {
      element.tabIndex = 0;
      element.setAttribute('role', 'button');
      element.setAttribute('aria-label', 'Edit text: ' + element.textContent);
    } else {
      element.removeAttribute('tabindex');
      element.removeAttribute('role');
      element.removeAttribute('aria-label');
    }
  }
  status.textContent = editing ? 'Select a text run.' : '';
});

function showEdit(selection, text) {
  dialog.querySelector('form').reset();
  selected = selection;
  const action = selection.action;
  const titles = {text: 'Replace text', format: 'Format text', paragraph: 'Add paragraph', outline: 'Add outline'};
  document.querySelector('#edit-heading').textContent = titles[action];
  document.querySelector('#edit-hint').textContent = {
    text: 'Replacement text keeps this run’s formatting.',
    format: 'Select part of the text or leave the whole run selected. Unchanged attributes keep their current values.',
    paragraph: 'Append to a container or insert before one of its children. Line breaks stay inside the new paragraph.',
    outline: 'Add a text container to this page. Line breaks stay inside its first paragraph.'
  }[action];
  for (const [id, visible] of [['placement', action === 'paragraph'], ['coordinates', action === 'outline'], ['formatting', action === 'format']]) {
    const field = document.getElementById(id);
    field.hidden = !visible;
    field.disabled = !visible;
  }
  document.querySelector('#author-label').hidden = !['paragraph', 'outline'].includes(action);
  draft.value = text;
  draft.readOnly = action === 'format';
  save.disabled = false;
  cancel.disabled = false;
  inspect.hidden = true;
  message.textContent = '';
  status.textContent = '';
  dialog.showModal();
  draft.focus();
  draft.setSelectionRange(0, text.length);
  document.querySelector('#format-range').textContent = 'Selected: ' + text.length + ' UTF-16 units';
  initial = JSON.stringify(requestBody());
}

async function openEdit(element) {
  if (opening) return;
  opening = true;
  status.textContent = 'Checking…';
  const selection = {generation: Number(route[1]), page: route[2],
    object: element.dataset.textObject, run: Number(element.dataset.run)};
  try {
    const result = await (await fetch('/api/run?' + new URLSearchParams(selection))).json();
    if (!result.ok) {
      status.textContent = 'Edit not supported. ' + result.error;
      return;
    }
    showEdit({...selection, action: document.querySelector('#text-action').value}, result.text);
  } catch {
    status.textContent = 'Unable to check this text. Reconnect to the diagnostic server.';
  } finally {
    opening = false;
  }
}

for (const action of ['paragraph', 'outline']) document.querySelector('#add-' + action).addEventListener('click', async () => {
  if (opening) return;
  opening = true;
  status.textContent = 'Loading…';
  const selection = {generation: Number(route[1]), page: route[2], action};
  try {
    const result = await (await fetch('/api/page?' + new URLSearchParams(selection))).json();
    if (!result.ok) throw new Error(result.error);
    if (action === 'paragraph' && !result.targets.length) throw new Error('Add an outline before adding a paragraph.');
    parent.replaceChildren(...result.targets.map(target => new Option(target.label, target.object)));
    parent.onchange = () => {
      const target = result.targets.find(target => target.object === parent.value);
      before.replaceChildren(new Option('Append at end', ''), ...(target?.children || []).map(child => new Option(child.label, child.object)));
    };
    parent.onchange();
    showEdit({...selection, object: result.object}, '');
  } catch (error) {
    status.textContent = 'Unable to add content. ' + error.message;
  } finally {
    opening = false;
  }
});

function requestBody() {
  const body = {...selected};
  if (body.action === 'text') body.replacement = draft.value;
  if (body.action === 'format') {
    body.start = draft.selectionStart;
    body.end = draft.selectionEnd;
    body.attributes = [...dialog.querySelectorAll('[data-attribute]')].filter(select => select.value !== '')
      .map(select => ({[select.dataset.attribute]: select.value === 'true'}));
    const font = document.querySelector('#font').value;
    const size = document.querySelector('#font-size').value;
    if (font) body.attributes.push({Font: font});
    if (size) body.attributes.push({FontSize: Number(size)});
    for (const [field, attribute] of [['color', 'Color'], ['highlight', 'Highlight']]) {
      const mode = document.getElementById(field + '-mode').value;
      if (mode !== 'keep') body.attributes.push({[attribute]: mode === 'clear' ? null : document.getElementById(field).value.slice(1).match(/../g).map(hex => parseInt(hex, 16))});
    }
  }
  if (['paragraph', 'outline'].includes(body.action)) {
    body.text = draft.value.replace(/\n/g, '\r');
    body.author = document.querySelector('#author').value;
    if (body.action === 'paragraph') {
      body.object = parent.value;
      body.before = before.value || null;
    } else {
      body.x = document.querySelector('#outline-x').valueAsNumber;
      body.y = document.querySelector('#outline-y').valueAsNumber;
    }
  }
  return body;
}

draft.addEventListener('select', () => {
  document.querySelector('#format-range').textContent = 'Selected: ' + (draft.selectionEnd - draft.selectionStart) + ' UTF-16 units';
});
document.addEventListener('click', event => {
  const element = event.target.closest('[data-text-object]');
  if (element && document.body.classList.contains('editing')) {
    event.preventDefault();
    openEdit(element);
  }
});
document.addEventListener('keydown', event => {
  if (event.target.matches('[data-text-object]') && document.body.classList.contains('editing') && ['Enter', ' '].includes(event.key)) {
    event.preventDefault();
    openEdit(event.target);
  }
});
cancel.addEventListener('click', () => dialog.close());
dialog.addEventListener('cancel', event => { if (submitting) event.preventDefault(); });
dialog.addEventListener('close', () => { selected = null; });
window.addEventListener('beforeunload', event => {
  if (selected && JSON.stringify(requestBody()) !== initial) event.preventDefault();
});
dialog.querySelector('form').addEventListener('submit', async event => {
  event.preventDefault();
  if (save.disabled || !selected) return;
  const body = requestBody();
  save.disabled = true;
  cancel.disabled = true;
  submitting = true;
  for (const control of dialog.querySelectorAll('input, select, textarea')) control.disabled = true;
  message.textContent = 'Saving…';
  let result;
  try {
    result = await (await fetch('/api/save', {method: 'POST', headers: {
      'Content-Type': 'application/json', 'X-OneNote-Diagnostic': '1'
    }, body: JSON.stringify(body)})).json();
  } catch {
    result = {ok: false, state: 'Unknown'};
  }
  submitting = false;
  cancel.disabled = false;
  for (const control of dialog.querySelectorAll('input, select, textarea')) control.disabled = false;
  if (result.ok) {
    selected = null;
    location.assign(result.location);
    return;
  }
  inspect.href = '/latest?' + new URLSearchParams({generation: selected.generation, page: selected.page});
  inspect.hidden = false;
  if (result.state === 'Unknown') {
    message.textContent = 'Save outcome unknown. Keep this draft and inspect the latest page before trying again.';
  } else if (result.state === 'Committed') {
    message.textContent = 'Saved, but cleanup or refresh did not finish. Inspect the latest page; do not save this draft again.';
  } else if (result.kind === 'ResourceBusy') {
    message.textContent = 'This section changed. Open the latest page before editing again. Your draft is still here.';
  } else {
    message.textContent = 'Unable to save. ' + (result.error || result.report_error || 'Check this edit and try again.');
    save.disabled = false;
  }
});
