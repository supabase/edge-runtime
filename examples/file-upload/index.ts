import { Application, Router } from 'https://deno.land/x/oak/mod.ts'
import { createClient } from 'https://esm.sh/@supabase/supabase-js@2'

const MB = 1024 * 1024

const app = new Application()

app.use(async (ctx) => {
  try {
    const body = ctx.request.body({ type: 'form-data' })
    const formData = await body.value.read({
      // Need to set the maxSize so files will be stored in memory.
      // This is necessary as Edge Functions don't have disk write access.
      // We are setting the max size as 10MB (an Edge Function has a max memory limit of 150MB)
      // For more config options, check: https://deno.land/x/oak@v11.1.0/mod.ts?s=FormDataReadOptions
      maxSize: 10 * MB,
    });
    const file = formData.files[0];
    console.log(file.contentType);

    ctx.response.status = 201
    ctx.response.body = 'Success!'
  } catch (e) {
    console.error(e)
    ctx.response.status = 500
    ctx.response.body = 'Error!'
  }
})

await app.listen({ port: 8000 })
