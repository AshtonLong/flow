/**
 * Must be the first import of the entry. Zod compiles object parsers with
 * `new Function` unless told not to, which the window's content security
 * policy forbids (and reports as a console error).
 */
import { z } from 'zod';

z.config({ jitless: true });
