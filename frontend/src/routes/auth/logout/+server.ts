import type { Cookies, RequestHandler } from '@sveltejs/kit';

import { resolveSafeReturnTarget } from '$lib/security/returnTarget';
import { SESSION_COOKIE_NAME } from '$lib/server/identity/session';

type LogoutInput = {
  url: URL;
  cookies: Pick<Cookies, 'delete'>;
};

export function logoutSession({ url, cookies }: LogoutInput): Response {
  cookies.delete(SESSION_COOKIE_NAME, { path: '/' });
  const returnTarget = resolveSafeReturnTarget(url.searchParams.get('return_to'));
  return new Response(null, {
    status: 303,
    headers: { Location: returnTarget, 'Cache-Control': 'no-store' }
  });
}

export const POST: RequestHandler = async ({ url, cookies }) => logoutSession({ url, cookies });
