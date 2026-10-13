import van from '../van-1.5.5.min.js';

const {header, a, span, div, button, section, h2, form, label, input, p} = van.tags;

export function SessionHeader(session) {
    return header({class: 'masthead'},
        a({class: 'brand', href: '/uploads/new'}, span({class: 'brand-mark', 'aria-hidden': 'true'}, 'b.'), ' booru-thingy'),
        div({class: 'account'}, span({id: 'account-name'}, session.name),
            button({id: 'logout', class: 'quiet', hidden: () => !session.authenticated.val,
                onclick: session.logout}, 'Sign out'),
            p({class: 'error', role: 'alert', hidden: () => !session.authenticated.val}, session.error)));
}

export function LoginPanel(session) {
    const login = input({name: 'login', autocomplete: 'username', required: true});
    const key = input({name: 'api_key', type: 'password', autocomplete: 'current-password', required: true});
    return section({id: 'login-panel', class: 'panel login', hidden: () => session.authenticated.val},
        h2('Sign in to upload'),
        form({id: 'login-form', onsubmit: async (event) => {
            event.preventDefault();
            if (await session.login({login: login.value, api_key: key.value})) event.target.reset();
        }}, label('Login', login), label('API key', key),
        button({class: 'primary', disabled: session.busy}, 'Sign in ', span({'aria-hidden': 'true'}, '↗')),
        p({id: 'login-error', class: 'error', role: 'alert'}, session.error)));
}
