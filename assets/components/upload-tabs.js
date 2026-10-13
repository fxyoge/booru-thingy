import van from '../van-1.5.5.min.js';

export function UploadTabs({mode, busy}) {
    const tabs = ['files', 'url'].map((value, index) => van.tags.button({
        id: index ? 'url-tab' : 'file-tab', type: 'button', role: 'tab',
        'aria-controls': index ? 'url-panel' : 'file-panel',
        'aria-selected': () => String(mode.val === value), disabled: busy,
        onclick: () => {if (!busy.val) mode.val = value;},
        onkeydown: (event) => {
            if (['ArrowLeft', 'ArrowRight'].includes(event.key)) {
                event.preventDefault();
                tabs[1 - index].click();
                tabs[1 - index].focus();
            }
        }
    }, index ? 'From a link' : 'Files'));
    return van.tags.div({class: 'tabs', role: 'tablist', 'aria-label': 'Upload method'}, tabs);
}
