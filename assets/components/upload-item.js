import van from '../van-1.5.5.min.js';

const {div, span, img, strong, small} = van.tags;
const labels = {receiving: 'Receiving', pending: 'Queued', processing: 'Processing', completed: 'Uploaded', error: 'Failed'};

export function UploadItem({item: initial, onRefreshPreview}) {
    const name = van.state(''), message = van.state(''), status = van.state('');
    const failed = van.state(false), src = van.state(null), visible = van.state(false);
    let assetKey, latestUrl = '', renewed = false;
    const media = img({'data-testid': 'upload-thumbnail', class: 'queue-thumb', alt: '', loading: 'lazy', decoding: 'async', hidden: () => !visible.val,
        onerror: () => {
            // Renew only a failed/expired lazy thumbnail, never an already-loaded image.
            if (latestUrl && src.val !== latestUrl) src.val = latestUrl;
            else if (latestUrl && !renewed) {renewed = true; onRefreshPreview();}
        }});
    // A missing src must be omitted; src="" could request the page itself.
    van.hydrate(media, (dom) => {
        if (src.val) dom.setAttribute('src', src.val);
        else dom.removeAttribute('src');
        return dom;
    });
    const element = div({'data-testid': 'upload-item', 'data-item-id': initial.id, class: 'queue-item'}, media,
        div({class: 'queue-text'}, strong(name), small(message)),
        span({class: () => `state${failed.val ? ' failed' : ''}`}, status));

    function update(item) {
        element.setAttribute('data-status', item.status);
        name.val = item.filename || item.source_url || 'URL import';
        const text = item.error || (item.post_id
            ? `${item.duplicate ? 'Already in your collection' : 'Uploaded'} · Post #${item.post_id}`
            : 'Processing');
        message.val = text + (item.status === 'error' && !item.source_url ? ' Select the file again to re-upload.' : '');
        failed.val = item.status === 'error';
        status.val = item.status === 'completed' && item.duplicate ? 'Duplicate' : labels[item.status] || item.status;
        const preview = item.media_asset?.variants?.find((variant) => variant.type === '180x180');
        latestUrl = preview?.url || '';
        // URL signatures can change while the underlying immutable object stays identical.
        const key = preview ? `${item.media_asset.id}:${new URL(preview.url, location.href).pathname}` : undefined;
        if (key !== assetKey) {assetKey = key; renewed = false; src.val = latestUrl || null;}
        visible.val = Boolean(preview);
        if (preview && media.complete && media.naturalWidth === 0 && src.val !== latestUrl) src.val = latestUrl;
    }
    update(initial);
    return {element, update};
}
