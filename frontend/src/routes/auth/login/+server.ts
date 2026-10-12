import { env } from '$env/dynamic/private';
import type { RequestHandler } from '@sveltejs/kit';

import { startLogin } from '$lib/server/identity/browserAuth';

export const GET: RequestHandler = async ({ url, cookies, fetch }) =>
  startLogin({ url, cookies, fetchFn: fetch, identityEnv: env });
