import { serve } from 'https://deno.land/std/http/server.ts'
import { corsHeaders } from 'shared_cors'

serve((req: Request) => {
  if (req.method === 'OPTIONS') {
    return new Response('ok', { headers: corsHeaders })
  }
  return new Response('Hello World', {
    headers: {
      ...corsHeaders,
      'content-type': 'text/plain',
    },
  })
})
