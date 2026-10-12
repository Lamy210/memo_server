// src/app.d.ts
/// <reference types="@sveltejs/kit" />
/// <reference types="svelte" />

declare global {
  namespace App {
    interface Locals {
      authenticated: boolean;
      accessToken?: string;
    }

    interface PageData {
      authenticated?: boolean;
      user?: {
        id: string;
        name: string;
        email: string;
      };
    }

    interface Error {
      message: string;
      code?: string;
    }

    interface Platform {}
  }
}

export {};
