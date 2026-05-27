// Auth.js v5 catch-all route handler. The actual configuration lives in
// `app/lib/auth.ts`; `handlers` is `{ GET, POST }`.
import { handlers } from '@/lib/auth';

export const { GET, POST } = handlers;
