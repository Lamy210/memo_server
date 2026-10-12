import { env } from '$env/dynamic/private';
import type { RequestHandler } from '@sveltejs/kit';

import { completeLogin } from '$lib/server/identity/browserAuth';

export const GET: RequestHandler = async ({ url, cookies, fetch }) =>
  completeLogin({ url, cookies, fetchFn: fetch, identityEnv: env });
