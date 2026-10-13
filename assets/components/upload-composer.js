import van from '../van-1.5.5.min.js';
import {api, json} from '../api.js';
import {createTransfers} from '../transfers.js';
import {FilePicker} from './file-picker.js';
import {UploadTabs} from './upload-tabs.js';
import {MetadataFields} from './metadata-fields.js';
import {TransferStatus} from './transfer-status.js';

const {section, div, h2, form, label, input, p, button, span} = van.tags;

export function UploadComposer({session, onUpload, maxBytes, sharedUrl = ''}) {
    const busy = van.state(false), error = van.state(''), mode = van.state(sharedUrl ? 'url' : 'files');
    const sourceUrl = van.state(sharedUrl);
    const picker = FilePicker({busy, maxBytes, onError: (message) => {error.val = message;},
        onSelect: () => {mode.val = 'files';}});
    const metadata = MetadataFields({busy, mode}), transferStatus = TransferStatus();
    const transfers = createTransfers(onUpload, transferStatus.update);

    async function submit(event) {
        event.preventDefault();
        if (busy.val) return;
        error.val = '';
        const files = picker.files();
        if (mode.val === 'files' && !files.length) {error.val = 'Choose at least one file first.'; return;}
        if (mode.val === 'url' && !sourceUrl.val.trim()) {error.val = 'Enter a direct media URL first.'; return;}
        const values = metadata.values();
        if (mode.val === 'url') values.source = sourceUrl.val;
        const signal = session.signal;
        busy.val = true;
        metadata.closeSuggestions();
        transferStatus.start();
        try {
            if (mode.val === 'files') await transfers.send(values, files, signal);
            else onUpload(await api('/uploads.json', {...json({upload: {
                source: values.source, tag_string: values.tags, rating: values.rating
            }}), signal}));
            if (signal.aborted) return;
            picker.clear();
            transfers.reset();
            sourceUrl.val = '';
            metadata.clear();
            transferStatus.done();
        } catch (reason) {
            if (!signal.aborted && reason.name !== 'AbortError') error.val = reason.message;
        } finally {if (!signal.aborted) busy.val = false;}
    }
    const element = section({class: 'panel composer'},
        div({class: 'section-heading'}, h2('Upload')), UploadTabs({mode, busy}),
        form({id: 'upload-form', onsubmit: submit},
            div({id: 'file-panel', role: 'tabpanel', 'aria-labelledby': 'file-tab', hidden: () => mode.val !== 'files'}, picker.element),
            div({id: 'url-panel', 'data-testid': 'url-panel', role: 'tabpanel', 'aria-labelledby': 'url-tab', hidden: () => mode.val !== 'url'},
                label('Direct media URL', input({id: 'source-url', 'data-testid': 'source-url', type: 'url', value: sourceUrl, disabled: busy,
                    placeholder: 'https://example.com/something-good.webp', oninput: (event) => {sourceUrl.val = event.target.value;}})),
                p({class: 'field-help'}, 'A link to the image or video itself, rather than a web page.')),
            metadata.element,
            div({class: 'submit-row'}, button({id: 'upload-button', 'data-testid': 'upload-submit', class: 'primary', type: 'submit', disabled: busy},
                'Upload', span({'aria-hidden': 'true'}, '↗'))),
            transferStatus.element, p({id: 'upload-error', 'data-testid': 'upload-error', class: 'error', role: 'alert'}, error)));

    document.addEventListener('paste', (event) => {
        if (session.authenticated.val && event.clipboardData.files.length) {
            event.preventDefault();
            picker.add(event.clipboardData.files);
        }
    });
    return {element, reset() {
        busy.val = false;
        error.val = '';
        picker.clear();
        metadata.clear();
        transfers.reset();
        transferStatus.reset();
        sourceUrl.val = '';
    }};
}
