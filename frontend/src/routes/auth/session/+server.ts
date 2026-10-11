import type { RequestHandler } from './$types';

export function getSessionStatus(authenticated: boolean): Response {
  return Response.json(
    { authenticated },
    { status: 200, headers: { 'Cache-Control': 'no-store' } }
  );
}

export const GET: RequestHandler = async ({ locals }) =>
  getSessionStatus(Boolean(locals.authenticated));
