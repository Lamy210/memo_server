import type { RequestHandler } from '@sveltejs/kit';

import { logoutSession } from '$lib/server/identity/browserAuth';

export const POST: RequestHandler = async ({ url, cookies }) => logoutSession({ url, cookies });
