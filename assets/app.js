import van from './van-1.5.5.min.js';
import {UploadPage} from './components/upload-page.js';

const sharedUrl = new URLSearchParams(location.search).get('url') || '';
const page = UploadPage({maxBytes: Number(document.body.dataset.maxBytes), sharedUrl});
van.add(document.getElementById('app'), page.element);
if (sharedUrl) history.replaceState(null, '', '/uploads/new');
page.restoreSession();
