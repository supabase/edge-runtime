import * as DenoCaches from 'ext:deno_cache/01_cache.js';
const core = globalThis.Deno.core;

const CACHE_NAMES_TO_INTERCEPT = ["transformers-cache"]

async function open(cacheName, next) {
  const cache = await next(cacheName);

  if(CACHE_NAMES_TO_INTERCEPT.includes(cacheName)) {
    const _cacheMatch = CachePrototype.match;

    cache.match = async function(args) {
		  return await match(args, (interceptedArgs) => _cacheMatch.call(cache, interceptedArgs));
	  };
  }

  return cache;
}

async function match(req, next) {
  const requestUrl = Request.prototype.isPrototypeOf(req) ? req.url() : req;
  if(!URL.canParse(requestUrl)) {
    console.log('not url', requestUrl);
    return undefined;
  }

  // TODO:(Cache all types): For now only intercept `.onnx` files
  if(!requestUrl.includes('onnx')){
    return await next(requestUrl);
  }

  console.log('intercepting onnx file fetch:', requestUrl);

  return new Response(requestUrl, { status: 200 });
}

const CacheStoragePrototype = DenoCaches.CacheStorage.prototype;
const CachePrototype = DenoCaches.Cache.prototype;

// TODO: Refactor to an `applyInterceptor` function
const _cacheStoragOpen = CacheStoragePrototype.open;
CacheStoragePrototype.open = async function(args) {
  return await open(args, (interceptedArgs) => _cacheStoragOpen.call(this, interceptedArgs));
};

