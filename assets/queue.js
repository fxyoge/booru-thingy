import van from './van-1.5.5.min.js';
import {api} from './api.js';

const active = (upload) => upload.status === 'processing';

// Network lifecycle/data model. Components own all DOM construction and reconciliation.
export function createUploadQueue({onUpsert, onRemove, onOrder}) {
    const uploads = new Map();
    const status = van.state(''), more = van.state(false), empty = van.state(true);
    let session, request, timer, limit = 10, listWanted = false, failures = 0;
    const hasActive = () => [...uploads.values()].some(active);

    function cancelRequest() {
        clearTimeout(timer);
        request?.abort();
        request = null;
    }
    function summarize() {
        empty.val = uploads.size === 0;
        status.val = hasActive() ? 'Processing in the background' : '';
    }
    function upsert(upload) {uploads.set(upload.id, upload); onUpsert(upload);}
    function remove(id) {uploads.delete(id); onRemove(id);}
    function order() {onOrder([...uploads.keys()].sort((a, b) => b - a));}
    function schedule(delay = 2000) {
        clearTimeout(timer);
        if (session && !document.hidden && (listWanted || hasActive())) timer = setTimeout(poll, delay);
    }
    async function poll() {
        if (!session || document.hidden || request) return;
        const current = new AbortController();
        request = current;
        try {
            if (listWanted) {
                const data = await api(`/users/1/uploads.json?limit=${limit}`, {signal: current.signal});
                if (request !== current) return;
                listWanted = false;
                const ids = new Set(data.map((upload) => upload.id));
                for (const id of uploads.keys()) if (!ids.has(id)) remove(id);
                data.forEach(upsert);
                more.val = data.length >= limit && limit < 200;
                order();
            } else {
                const ids = [...uploads.values()].filter(active).map((upload) => upload.id);
                let cursor = 0;
                const worker = async () => {
                    while (cursor < ids.length && request === current) {
                        const upload = await api(`/uploads/${ids[cursor++]}`, {signal: current.signal});
                        if (request === current) upsert(upload);
                    }
                };
                await Promise.all([worker(), worker()]);
            }
            if (request !== current) return;
            failures = 0;
            summarize();
        } catch (error) {
            if (request !== current || error.name === 'AbortError') return;
            current.abort();
            failures++;
            status.val = error.message;
        } finally {
            if (request === current) {request = null; schedule(Math.min(30000, 2000 * 2 ** failures));}
        }
    }
    function accept(upload) {
        if (!session) return;
        // Mutation results supersede older polling snapshots that may still be in flight.
        cancelRequest();
        upsert(upload);
        order();
        summarize();
        schedule();
    }
    function refresh() {
        if (!session) return;
        cancelRequest();
        listWanted = true;
        poll();
    }
    async function loadOne(id, retry = false) {
        const current = session;
        if (!current) return;
        try {
            const data = await api(`/uploads/${id}${retry ? '/retry' : ''}`, {
                method: retry ? 'POST' : 'GET', signal: current.signal
            });
            if (session === current) accept(data);
        } catch (error) {if (session === current && error.name !== 'AbortError') status.val = error.message;}
    }
    function stop() {
        cancelRequest();
        session?.abort();
        session = null;
        for (const id of uploads.keys()) remove(id);
        limit = 10;
        listWanted = false;
        failures = 0;
        more.val = false;
        summarize();
    }
    return {
        status, more, empty, accept, stop,
        start() {stop(); session = new AbortController(); refresh();},
        loadMore() {limit = Math.min(limit + 10, 200); refresh();},
        retry: (id) => loadOne(id, true),
        refreshPreview: (id) => loadOne(id),
        visibilityChanged() {if (document.hidden) cancelRequest(); else refresh();}
    };
}
