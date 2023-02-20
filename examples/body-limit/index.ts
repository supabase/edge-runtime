import { Application, Router, isHttpError } from 'oak'
const app = new Application()
const router = new Router()

// introduce middleware to handle all exceptions, otherwise api will only return "internal error"
app.use(async (context, next) => {
  try {
    await next()
  } catch (err) {
    console.error(err.message)
    context.response.status = isHttpError(err) ? err.status : 500
    context.response.body = err.message
  }
})

router.post('/body-limit', async (ctx) => {
  console.log(ctx.request.headers.get('content-length'));
  const body = ctx.request.body({type: "json"})
  console.log(body)
  const value = await body.value
  console.log(value)
  ctx.response.body = value
})

app.use(router.routes())
app.use(router.allowedMethods())

await app.listen({ port: 8000 })
