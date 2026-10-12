import { dev } from '$app/environment';
import { env } from '$env/dynamic/private';
import type { LayoutServerLoad } from './$types';

export const load: LayoutServerLoad = async ({ locals }) => ({
  authenticated: dev
    ? Boolean(env.DEVELOPMENT_USER_ID?.trim())
    : Boolean(locals.authenticated)
});
