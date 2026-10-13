import van from './van-1.5.5.min.js';
import {api, configureSession, json} from './api.js';

export function createSession({onSignIn, onSignOut}) {
    const authenticated = van.state(false), name = van.state(''), error = van.state(''), busy = van.state(false);
    let controller = new AbortController(), authentication;

    function signOut() {
        const wasAuthenticated = authenticated.val;
        controller.abort();
        controller = new AbortController();
        configureSession('', signOut);
        authenticated.val = false;
        name.val = '';
        if (wasAuthenticated) onSignOut();
    }
    function signIn(data) {
        configureSession(data.csrf_token, signOut);
        authenticated.val = true;
        name.val = data.name;
        error.val = '';
        onSignIn();
    }
    async function authenticate(path, options = {}) {
        authentication?.abort();
        const current = new AbortController();
        authentication = current;
        try {
            const data = await api(path, {...options, signal: current.signal});
            if (authentication === current) signIn(data);
            return authentication === current;
        } catch (reason) {
            if (reason.name !== 'AbortError' && authentication === current) {
                if (path === '/session') signOut();
                else error.val = reason.message;
            }
            return false;
        }
    }
    configureSession('', signOut);
    return {
        authenticated, name, error, busy,
        get signal() {return controller.signal;},
        restore: () => authenticate('/session'),
        async login(credentials) {
            if (busy.val) return false;
            busy.val = true;
            error.val = '';
            try {return await authenticate('/login', json(credentials));}
            finally {busy.val = false;}
        },
        async logout() {
            try {await api('/session', {method: 'DELETE', signal: controller.signal}); signOut();}
            catch (reason) {if (reason.name !== 'AbortError') error.val = reason.message;}
        }
    };
}
