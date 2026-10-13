let csrf = '';
let onUnauthorized = () => {};

export function configureSession(token, unauthorized) {
    csrf = token || '';
    onUnauthorized = unauthorized;
}

export async function api(path, options = {}) {
    const response = await fetch(path, {
        ...options,
        headers: {...options.headers, 'X-CSRF-Token': csrf}
    });
    const data = await response.json().catch(() => ({reason: 'Unexpected server response'}));
    options.signal?.throwIfAborted();
    if (!response.ok) {
        if (response.status === 401) onUnauthorized();
        const error = new Error(data.reason || `Request failed (${response.status})`);
        error.status = response.status;
        throw error;
    }
    return data;
}

export const json = (body) => ({
    method: 'POST',
    headers: {'Content-Type': 'application/json'},
    body: JSON.stringify(body)
});
