Deno.serve(() => {
  Promise.reject(new Error('unhandled rejection regression fixture'));
  return new Promise<Response>(() => {});
});
