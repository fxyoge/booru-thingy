import van from '../van-1.5.5.min.js';
import {TagInput} from './tag-input.js';

const {div, label, span, input, fieldset, legend} = van.tags;

export function MetadataFields({busy, mode}) {
    const tags = van.state(''), source = van.state(''), rating = van.state('g');
    const tagInput = TagInput({value: tags, busy});
    const element = div({class: 'metadata'}, tagInput.element,
        fieldset(legend('Rating'), div({class: 'rating'},
            [['g', 'General'], ['s', 'Sensitive'], ['q', 'Questionable'], ['e', 'Explicit']].map(([value, title]) =>
                label(input({'data-testid': `rating-${value}`, type: 'radio', name: 'rating', value, disabled: busy,
                    checked: () => rating.val === value, onchange: () => {rating.val = value;}}), span(title))))),
        label({id: 'source-label', 'data-testid': 'source-label', hidden: () => mode.val === 'url'}, 'Source ', span({class: 'optional'}, 'optional'),
            input({id: 'source', 'data-testid': 'source', type: 'url', value: source, disabled: busy, placeholder: 'Where did you find it?',
                oninput: (event) => {source.val = event.target.value;}})));
    return {
        element, closeSuggestions: tagInput.close,
        values: () => ({tags: tags.val, source: source.val, rating: rating.val}),
        clear() {tags.val = ''; source.val = ''; tagInput.close();}
    };
}
