import van from '../van-1.5.5.min.js';
import {api} from '../api.js';

const {label, span, input, div, button, small, code} = van.tags;

export function TagInput({value, busy}) {
    const open = van.state(false);
    let request, timer;
    const suggestions = div({id: 'suggestions', class: 'suggestions', hidden: () => !open.val});
    const field = input({id: 'tags', autocomplete: 'off', placeholder: 'cat very_good_cat', value, disabled: busy,
        'aria-controls': 'suggestions', 'aria-expanded': () => String(open.val),
        oninput: (event) => {value.val = event.target.value; search();},
        onkeydown: (event) => {
            if (event.key === 'Escape') close();
            if (event.key === 'ArrowDown') {suggestions.firstElementChild?.focus(); event.preventDefault();}
        }});
    const element = label({class: 'tags-label'}, 'Tags ', span({class: 'optional'}, 'optional'), field, suggestions,
        span({class: 'field-help'}, 'Separate tags with spaces. Leave blank for ', code('tagme'), '.'));

    function close() {
        clearTimeout(timer);
        request?.abort();
        request = null;
        open.val = false;
    }
    function search() {
        close();
        const prefix = value.val.split(/\s+/).pop();
        if (!prefix) return;
        const current = new AbortController();
        request = current;
        timer = setTimeout(async () => {
            try {
                const tags = await api(`/autocomplete.json?search[query]=${encodeURIComponent(prefix)}&limit=6`, {signal: current.signal});
                if (request !== current) return;
                suggestions.replaceChildren(...tags.map((tag) => button({type: 'button', onclick: () => {
                    value.val = value.val.replace(/\S*$/, tag.value + ' ');
                    close();
                    field.focus();
                }}, tag.value, small(tag.post_count))));
                open.val = Boolean(tags.length);
            } catch (_) { /* Suggestions are optional; upload errors belong to the composer. */ }
        }, 180);
    }
    document.addEventListener('click', (event) => {if (!element.contains(event.target)) close();});
    return {element, close};
}
