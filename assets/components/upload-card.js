import van from '../van-1.5.5.min.js';
import {UploadItem} from './upload-item.js';

const {div, span, p, button} = van.tags;

export function UploadCard({upload: initial, onRetry, onRefreshPreview}) {
    const items = new Map(), empty = van.state(false), retryable = van.state(false), retrying = van.state(false);
    const rows = div();
    const element = div({class: 'queue-card'},
        div({class: 'queue-title'}, span(`Upload #${initial.id}`), span(new Date(initial.created_at).toLocaleString())), rows,
        p({class: 'field-help', hidden: () => !empty.val}, 'Upload was interrupted before any files were received.'),
        button({type: 'button', class: 'quiet', hidden: () => !retryable.val, disabled: retrying, onclick: async () => {
            if (retrying.val) return;
            retrying.val = true;
            try {await onRetry(initial.id);}
            finally {retrying.val = false;}
        }}, 'Retry failed items'));

    function update(upload) {
        const ids = new Set(upload.upload_media_assets.map((item) => item.id));
        for (const [id, component] of items) if (!ids.has(id)) {
            component.element.remove();
            items.delete(id);
        }
        for (const item of upload.upload_media_assets) {
            if (items.has(item.id)) items.get(item.id).update(item);
            else {
                const component = UploadItem({item, onRefreshPreview: () => onRefreshPreview(initial.id)});
                items.set(item.id, component);
                van.add(rows, component.element);
            }
        }
        empty.val = !ids.size;
        retryable.val = upload.upload_media_assets.some((item) => item.status === 'error' && item.source_url);
    }
    update(initial);
    return {element, update};
}
