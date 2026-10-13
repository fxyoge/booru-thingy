import van from '../van-1.5.5.min.js';
import {createSession} from '../session.js';
import {SessionHeader, LoginPanel} from './session-ui.js';
import {UploadComposer} from './upload-composer.js';
import {RecentUploads} from './recent-uploads.js';

export function UploadPage({maxBytes, sharedUrl}) {
    const recent = RecentUploads();
    const session = createSession({onSignIn: recent.start, onSignOut: () => {
        recent.stop();
        composer.reset();
    }});
    const composer = UploadComposer({session, maxBytes, sharedUrl, onUpload: recent.accept});
    return {
        element: van.tags.div(SessionHeader(session), van.tags.main(
            LoginPanel(session),
            van.tags.div({id: 'workspace', 'data-testid': 'workspace', hidden: () => !session.authenticated.val}, composer.element, recent.element))),
        restoreSession: session.restore
    };
}
