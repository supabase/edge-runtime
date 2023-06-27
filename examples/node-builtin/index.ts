import { randomBytes } from "node:crypto";
import { serve } from "https://deno.land/std@0.131.0/http/server.ts"

const generateRandomString = (length) => {
    const buffer = randomBytes(length);
    return buffer.toString('hex');
};

const randomString = generateRandomString(10);
console.log(randomString);

serve(async (req: Request) => {
    return new Response(generateRandomString(10), { status: 200 })
});
