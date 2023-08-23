import Logger from "https://deno.land/x/logger@v1.1.1/logger.ts";
import { serve } from "https://deno.land/std@0.131.0/http/server.ts"

const logger = new Logger();

serve(async (req: Request) => {
    logger.info("hello from logger");

    return new Response(
        JSON.stringify({ hello: "world" }),
        { status: 200, headers: { "Content-Type": "application/json" } },
    )
})