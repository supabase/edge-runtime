import { Hono } from 'https://deno.land/x/hono@v3.0.1/mod.ts'
import { serve } from 'https://deno.land/std@0.168.0/http/server.ts'

console.log('Notion Parser Method Functions!')

const app = new Hono()

app.get('/hono', (c) => c.text('Hello Deno 123!'))

serve(app.fetch)
