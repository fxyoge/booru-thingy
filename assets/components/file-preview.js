import van from '../van-1.5.5.min.js';
import {size} from '../format.js';

const {div, img, video, p, button} = van.tags;

export function FilePreview({file, url, busy, onRemove}) {
    const media = file.type.startsWith('video/')
        ? video({src: url, muted: true, preload: 'metadata'})
        : img({src: url, alt: file.name, decoding: 'async'});
    return div({class: 'preview'}, media, p(`${file.name} · ${size(file.size)}`),
        button({type: 'button', class: 'remove', disabled: busy,
            'aria-label': `Remove ${file.name}`, onclick: onRemove}, '×'));
}
