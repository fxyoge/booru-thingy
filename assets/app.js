'use strict';
const $ = (id) => document.getElementById(id);
let csrf = '', selected = [], mode = 'files', busy = false, queueLimit = 10;
const maxBytes = Number(document.body.dataset.maxBytes);
const size = (bytes) => bytes >= 1048576 ? `${(bytes / 1048576).toFixed(1)} MB` : `${Math.ceil(bytes / 1024)} KB`;
$('size-limit').textContent = `· ${size(maxBytes)} per batch`;
function node(tag, text, className) { const el = document.createElement(tag); if (text !== undefined) el.textContent = text; if (className) el.className = className; return el; }
async function api(path, options = {}) {
  const response = await fetch(path, { ...options, headers: { ...options.headers, 'X-CSRF-Token': csrf } });
  const data = await response.json().catch(() => ({ reason: 'Unexpected server response' }));
  if (!response.ok) { if (response.status === 401) signedOut(); throw new Error(data.reason || `Request failed (${response.status})`); }
  return data;
}
function signedOut() { $('workspace').hidden = true; $('login-panel').hidden = false; $('logout').hidden = true; $('account-name').textContent = ''; }
function signedIn(data) { csrf = data.csrf_token || ''; $('workspace').hidden = false; $('login-panel').hidden = true; $('logout').hidden = false; $('account-name').textContent = data.name; refresh(); }
$('login-form').addEventListener('submit', async (event) => {
  event.preventDefault(); const button = event.submitter; button.disabled = true; $('login-error').textContent = '';
  try { const values = Object.fromEntries(new FormData(event.target)); const data = await api('/login', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(values) }); event.target.reset(); signedIn(data); }
  catch (error) { $('login-error').textContent = error.message; } finally { button.disabled = false; }
});
$('logout').addEventListener('click', async () => { try { await api('/session', { method: 'DELETE' }); signedOut(); } catch (error) { $('queue-status').textContent = error.message; } });
function setMode(next) {
  if (busy) return; mode = next;
  $('file-panel').hidden = next !== 'files'; $('url-panel').hidden = next !== 'url'; $('source-label').hidden = next === 'url';
  $('file-tab').setAttribute('aria-selected', String(next === 'files')); $('url-tab').setAttribute('aria-selected', String(next === 'url'));
}
$('file-tab').addEventListener('click', () => setMode('files'));
$('url-tab').addEventListener('click', () => setMode('url'));
for (const [id, other] of [['file-tab', 'url-tab'], ['url-tab', 'file-tab']]) $(id).addEventListener('keydown', (event) => { if (['ArrowLeft', 'ArrowRight'].includes(event.key)) { event.preventDefault(); $(other).click(); $(other).focus(); } });
function addFiles(files) {
  if (busy) return;
  $('upload-error').textContent = '';
  const supported = /^(image\/(jpeg|png|gif|webp)|video\/(mp4|webm))$/;
  for (const file of files) {
    if (!supported.test(file.type)) { $('upload-error').textContent = `${file.name}: unsupported file type`; continue; }
    if (selected.length >= 32 || selected.reduce((sum, item) => sum + item.file.size, 0) + file.size > maxBytes) { $('upload-error').textContent = 'This batch exceeds the file count or size limit.'; break; }
    selected.push({ file, url: URL.createObjectURL(file) });
  }
  setMode('files'); previews(); $('files').value = '';
}
function previews() {
  $('previews').replaceChildren();
  selected.forEach((item, index) => {
    const card = node('div', undefined, 'preview'); const media = node(item.file.type.startsWith('video/') ? 'video' : 'img'); media.src = item.url; media.alt = item.file.name; if (media.tagName === 'VIDEO') { media.muted = true; media.preload = 'metadata'; }
    const remove = node('button', '×', 'remove'); remove.type = 'button'; remove.disabled = busy; remove.setAttribute('aria-label', `Remove ${item.file.name}`); remove.addEventListener('click', () => { URL.revokeObjectURL(item.url); selected.splice(index, 1); previews(); });
    card.append(media, node('p', `${item.file.name} · ${size(item.file.size)}`), remove); $('previews').append(card);
  });
}
$('files').addEventListener('change', (event) => addFiles(event.target.files));
for (const type of ['dragenter', 'dragover']) $('dropzone').addEventListener(type, (event) => { event.preventDefault(); $('dropzone').classList.add('drag'); });
for (const type of ['dragleave', 'drop']) $('dropzone').addEventListener(type, (event) => { event.preventDefault(); $('dropzone').classList.remove('drag'); });
$('dropzone').addEventListener('drop', (event) => addFiles(event.dataTransfer.files));
document.addEventListener('paste', (event) => { if (!$('workspace').hidden && event.clipboardData.files.length) { event.preventDefault(); addFiles(event.clipboardData.files); } });
let suggestionRequest = 0;
$('tags').addEventListener('input', async () => {
  const request = ++suggestionRequest; const prefix = $('tags').value.split(/\s+/).pop();
  $('suggestions').hidden = true; $('tags').setAttribute('aria-expanded', 'false');
  if (!prefix) return;
  await new Promise((resolve) => setTimeout(resolve, 180)); if (request !== suggestionRequest) return;
  try {
    const suggestions = await api(`/autocomplete.json?search[query]=${encodeURIComponent(prefix)}&limit=6`); if (request !== suggestionRequest) return;
    $('suggestions').replaceChildren();
    suggestions.forEach((tag) => { const button = node('button', tag.value); button.type = 'button'; button.append(node('small', tag.post_count)); button.addEventListener('click', () => { $('tags').value = $('tags').value.replace(/\S*$/, tag.value + ' '); $('suggestions').hidden = true; $('tags').setAttribute('aria-expanded', 'false'); $('tags').focus(); }); $('suggestions').append(button); });
    $('suggestions').hidden = !suggestions.length; $('tags').setAttribute('aria-expanded', String(Boolean(suggestions.length)));
  } catch (_) { /* Uploads remain usable if suggestions are temporarily unavailable. */ }
});
$('tags').addEventListener('keydown', (event) => { if (event.key === 'Escape') { $('suggestions').hidden = true; $('tags').setAttribute('aria-expanded', 'false'); } if (event.key === 'ArrowDown') { $('suggestions').querySelector('button')?.focus(); event.preventDefault(); } });
document.addEventListener('click', (event) => { if (!event.target.closest('.tags-label')) { $('suggestions').hidden = true; $('tags').setAttribute('aria-expanded', 'false'); } });
$('upload-form').addEventListener('submit', async (event) => {
  event.preventDefault(); if (busy) return; $('upload-error').textContent = '';
  if (mode === 'files' && !selected.length) { $('upload-error').textContent = 'Choose at least one file first.'; return; }
  if (mode === 'url' && !$('source-url').value.trim()) { $('upload-error').textContent = 'Enter a direct media URL first.'; return; }
  const form = new FormData(); form.append('upload[source]', mode === 'url' ? $('source-url').value : $('source').value); form.append('upload[tag_string]', $('tags').value); form.append('upload[rating]', document.querySelector('[name=rating]:checked').value);
  if (mode === 'files') selected.forEach(({ file }) => form.append('upload[files][]', file));
  busy = true;[...$('upload-form').elements].forEach((el) => el.disabled = true); $('transfer').hidden = false; $('progress').value = 0; previews(); $('transfer-label').textContent = 'Uploading…';
  try {
    await new Promise((resolve, reject) => {
      const xhr = new XMLHttpRequest(); xhr.open('POST', '/uploads.json'); xhr.setRequestHeader('X-CSRF-Token', csrf); xhr.timeout = 600000;
      xhr.upload.onprogress = (event) => { if (event.lengthComputable) { $('progress').value = event.loaded / event.total * 100; $('transfer-label').textContent = event.loaded === event.total ? 'Saving your upload…' : `Uploading ${Math.round($('progress').value)}%`; } };
      xhr.onload = () => { let data; try { data = JSON.parse(xhr.responseText); } catch (_) { reject(new Error('Unexpected server response')); return; } if (xhr.status >= 200 && xhr.status < 300) resolve(data); else { if (xhr.status === 401) signedOut(); reject(new Error(data.reason || 'Upload failed')); } };
      xhr.onerror = () => reject(new Error('Connection interrupted. Check recent uploads before trying again.')); xhr.ontimeout = () => reject(new Error('Upload timed out. Check recent uploads before trying again.')); xhr.send(form);
    });
    selected.forEach((item) => URL.revokeObjectURL(item.url)); selected = []; $('source-url').value = ''; $('source').value = ''; $('tags').value = ''; $('transfer-label').textContent = 'Received. Processing continues in the background.'; refresh();
  } catch (error) { $('upload-error').textContent = error.message; } finally { busy = false;[...$('upload-form').elements].forEach((el) => el.disabled = false); previews(); }
});
let refreshing = false;
async function refresh() {
  if (refreshing || $('workspace').hidden) return; refreshing = true;
  try {
    const uploads = await api(`/users/1/uploads.json?limit=${queueLimit}`); const queue = $('queue');
    $('queue-status').textContent = uploads.some((u) => u.status === 'processing') ? 'Processing in the background' : ''; $('more').hidden = uploads.length < queueLimit || queueLimit >= 200;
    if (!uploads.length) return;
    queue.replaceChildren();
    for (const upload of uploads) {
      const card = node('div', undefined, 'queue-card'); const title = node('div', undefined, 'queue-title'); title.append(node('span', `Upload #${upload.id}`), node('span', new Date(upload.created_at).toLocaleString())); card.append(title);
      for (const item of upload.upload_media_assets) {
        const row = node('div', undefined, 'queue-item'); const thumb = node('img', undefined, 'queue-thumb'); const preview = item.media_asset?.variants?.find((v) => v.type === '180x180'); if (preview) { thumb.src = preview.url; thumb.alt = ''; } else { thumb.hidden = true; }
        const text = node('div', undefined, 'queue-text'); text.append(node('strong', item.filename || item.source_url || 'URL import')); text.append(node('small', item.error || (item.post_id ? `${item.duplicate ? 'Already in your collection' : 'Published'} · Post #${item.post_id}` : 'We’re getting it ready.')));
        const labels = { receiving: 'Receiving', pending: 'Queued', processing: 'Processing', completed: item.duplicate ? 'Duplicate' : 'Published', error: 'Failed' }; row.append(thumb, text, node('span', labels[item.status] || item.status, `state${item.status === 'error' ? ' failed' : ''}`)); card.append(row);
      }
      if (!upload.upload_media_assets.length) card.append(node('p', 'Upload was interrupted before any files were received.', 'field-help'));
      if (upload.status === 'error' || upload.upload_media_assets.some((item) => item.status === 'error')) {
        const retry = node('button', 'Retry failed items', 'quiet'); retry.addEventListener('click', async () => { retry.disabled = true; try { await api(`/uploads/${upload.id}/retry`, { method: 'POST' }); refresh(); } catch (error) { $('queue-status').textContent = error.message; } finally { retry.disabled = false; } }); card.append(retry);
      }
      queue.append(card);
    }
  } catch (error) { $('queue-status').textContent = error.message; } finally { refreshing = false; }
}
$('more').addEventListener('click', () => { queueLimit = Math.min(queueLimit + 10, 200); refresh(); });
const shared = new URLSearchParams(location.search).get('url'); if (shared) { setMode('url'); $('source-url').value = shared; history.replaceState(null, '', '/uploads/new'); }
api('/session').then(signedIn).catch(signedOut);
setInterval(() => { if (!document.hidden) refresh(); }, 4000);
