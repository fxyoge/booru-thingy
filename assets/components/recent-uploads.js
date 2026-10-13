import van from '../van-1.5.5.min.js';
import {createUploadQueue} from '../queue.js';
import {UploadCard} from './upload-card.js';

const {section, div, h2, span, p, button} = van.tags;

export function RecentUploads() {
    const cards = new Map(), container = div({id: 'queue'});
    const queue = createUploadQueue({
        onUpsert(upload) {
            if (cards.has(upload.id)) cards.get(upload.id).update(upload);
            else {
                const card = UploadCard({upload, onRetry: queue.retry, onRefreshPreview: queue.refreshPreview});
                cards.set(upload.id, card);
                van.add(container, card.element);
            }
        },
        onRemove(id) {cards.get(id)?.element.remove(); cards.delete(id);},
        onOrder(ids) {
            // Only move cards whose position changes; retain all unchanged media nodes.
            let next = null;
            for (const id of [...ids].reverse()) {
                const element = cards.get(id).element;
                if (element.nextSibling !== next) container.insertBefore(element, next);
                next = element;
            }
        }
    });
    van.add(container, div({class: 'empty-state', hidden: () => !queue.empty.val},
        span({'aria-hidden': 'true'}, '✧'), p('No recent uploads.')));
    const element = section({class: 'recent'},
        div({class: 'section-heading'}, h2('Recent uploads'),
            span({id: 'queue-status', class: 'muted', role: 'status'}, queue.status)), container,
        button({id: 'more', class: 'quiet', hidden: () => !queue.more.val, onclick: queue.loadMore}, 'Show older uploads'));
    document.addEventListener('visibilitychange', queue.visibilityChanged);
    return {element, accept: queue.accept, start: queue.start, stop: queue.stop};
}
