import van from '../van-1.5.5.min.js';
import {size} from '../format.js';
import {FilePreview} from './file-preview.js';

const {div, label, input, span, strong, small} = van.tags;
const supported = /^(image\/(jpeg|png|gif|webp)|video\/(mp4|webm))$/;

export function FilePicker({busy, maxBytes, onError, onSelect}) {
    let selected = [];
    const dragging = van.state(false);
    const previews = div({id: 'previews', class: 'previews', 'aria-label': 'Selected files'});
    const chooser = input({id: 'files', 'data-testid': 'files', type: 'file', multiple: true, disabled: busy,
        accept: 'image/jpeg,image/png,image/gif,image/webp,video/mp4,video/webm',
        onchange: (event) => {add(event.target.files); event.target.value = '';}});
    const drag = (event) => {event.preventDefault(); dragging.val = true;};
    const element = div(
        label({id: 'dropzone', class: () => `dropzone${dragging.val ? ' drag' : ''}`,
            ondragenter: drag, ondragover: drag,
            ondragleave: (event) => {event.preventDefault(); dragging.val = false;},
            ondrop: (event) => {event.preventDefault(); dragging.val = false; add(event.dataTransfer.files);}},
            chooser, span({class: 'upload-symbol', 'aria-hidden': 'true'}, '↑'), strong('Drop your files here'),
            span('or ', span({class: 'text-link'}, 'choose files'), ' · paste works too'),
            small('JPG, PNG, GIF, WebP, MP4 & WebM ', span({id: 'size-limit'}, `· ${size(maxBytes)} per batch`))),
        previews);

    function remove(item) {
        if (busy.val) return;
        item.element.remove();
        URL.revokeObjectURL(item.url);
        selected = selected.filter((other) => other !== item);
    }
    function add(files) {
        if (busy.val) return;
        onError('');
        let bytes = selected.reduce((sum, item) => sum + item.file.size, 0);
        for (const file of files) {
            if (!supported.test(file.type)) {onError(`${file.name}: unsupported file type`); continue;}
            if (selected.length >= 32 || bytes + file.size > maxBytes) {
                onError('This batch exceeds the file count or size limit.');
                break;
            }
            bytes += file.size;
            const item = {file, url: URL.createObjectURL(file)};
            item.element = FilePreview({...item, busy, onRemove: () => remove(item)});
            selected.push(item);
            van.add(previews, item.element);
        }
        onSelect();
    }
    function clear() {
        for (const item of selected) {
            item.element.remove();
            URL.revokeObjectURL(item.url);
        }
        selected = [];
        dragging.val = false;
    }
    return {element, add, clear, files: () => selected};
}
