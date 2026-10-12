import type { RequestHandler } from './$types';

import { getSessionStatus } from '$lib/server/identity/browserAuth';

export const GET: RequestHandler = async ({ locals }) =>
  getSessionStatus(Boolean(locals.authenticated));
