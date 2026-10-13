import van from '../van-1.5.5.min.js';

export function TransferStatus() {
    const visible = van.state(false), percent = van.state(0), message = van.state('');
    return {
        element: van.tags.div({id: 'transfer', hidden: () => !visible.val},
            van.tags.progress({id: 'progress', max: 100, value: percent}),
            van.tags.p({id: 'transfer-label', class: 'field-help', role: 'status'}, message)),
        start() {visible.val = true; percent.val = 0; message.val = 'Uploading...';},
        update(value, label) {percent.val = value; message.val = label;},
        done() {percent.val = 100; message.val = 'Done.';},
        reset() {visible.val = false; percent.val = 0; message.val = '';}
    };
}
