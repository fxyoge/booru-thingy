import {api, json} from './api.js';

function sendFile(ticket, file, progress, signal) {
    return new Promise((resolve, reject) => {
        const form = new FormData();
        Object.entries(ticket.fields).forEach(([key, value]) => form.append(key, value));
        form.append('file', file); // S3 requires the file part last.
        const xhr = new XMLHttpRequest();
        xhr.open('POST', ticket.url);
        xhr.timeout = 900000;
        const abort = () => xhr.abort();
        const finish = (error, etag) => {
            signal.removeEventListener('abort', abort);
            if (error) reject(error); else resolve(etag);
        };
        xhr.upload.onprogress = (event) => {
            if (event.lengthComputable) progress(Math.min(file.size, event.loaded / event.total * file.size));
        };
        xhr.onload = () => {
            if (xhr.status < 200 || xhr.status >= 300) {
                finish(new Error(`Storage upload failed (${xhr.status}). Check storage CORS and upload policy.`));
            } else {
                const etag = xhr.getResponseHeader('ETag');
                finish(etag ? null : new Error('Storage did not expose an ETag. Check storage CORS exposes ETag.'), etag);
            }
        };
        xhr.onerror = () => finish(new Error('Storage connection interrupted. Check storage CORS and connectivity.'));
        xhr.ontimeout = () => finish(new Error('Storage transfer timed out.'));
        xhr.onabort = () => finish(new DOMException('Transfer cancelled', 'AbortError'));
        if (signal.aborted) { finish(new DOMException('Transfer cancelled', 'AbortError')); return; }
        signal.addEventListener('abort', abort, {once: true});
        xhr.send(form); // No API credentials or cookies are sent to storage.
    });
}

export function createTransfers(accept, progress) {
    let batch = null;
    return {
        reset() {batch = null;},
        async send(metadata, selected, signal) {
            const fingerprint = JSON.stringify([metadata, selected.map((item) => item.url)]);
            if (!batch || batch.fingerprint !== fingerprint) {
                const upload = await api('/uploads/direct', {...json({
                    ...metadata, files: selected.map(({file}) => ({filename: file.name, size: file.size}))
                }), signal});
                accept(upload);
                batch = {fingerprint, id: upload.id, items: upload.upload_media_assets.map((item, index) => ({
                    id: item.id, file: selected[index].file, loaded: 0, etag: null, completed: false
                }))};
            }
            const current = batch, errors = [], total = current.items.reduce((sum, item) => sum + item.file.size, 0);
            let cursor = 0;
            const update = () => {
                const percent = total ? Math.round(current.items.reduce((sum, item) => sum + item.loaded, 0) / total * 100) : 100;
                progress(percent, `Uploading ${percent}% · ${current.items.filter((item) => item.completed).length}/${current.items.length} uploaded`);
            };
            update();
            const run = async () => {
                while (cursor < current.items.length && !signal.aborted) {
                    const item = current.items[cursor++];
                    if (item.completed) continue;
                    const path = `/uploads/${current.id}/items/${item.id}`;
                    try {
                        if (!item.etag) {
                            item.loaded = 0;
                            const ticket = await api(`${path}/ticket`, {method: 'POST', signal});
                            item.etag = await sendFile(ticket, item.file, (loaded) => {item.loaded = loaded; update();}, signal);
                            item.loaded = item.file.size;
                        }
                        const upload = await api(`${path}/complete`, {...json({etag: item.etag}), signal});
                        item.completed = true;
                        update();
                        accept(upload);
                    } catch (error) {
                        if (signal.aborted) throw error;
                        if (error.status === 409) item.etag = null;
                        errors.push(`${item.file.name}: ${error.message}`);
                    }
                }
            };
            await Promise.all([run(), run()]);
            signal.throwIfAborted();
            if (errors.length) throw new Error(`${errors.join(' ')} Click Upload to retry unfinished files.`);
            batch = null;
        }
    };
}
