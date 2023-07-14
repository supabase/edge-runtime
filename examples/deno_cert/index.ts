import {serve} from "https://deno.land/std@0.131.0/http/server.ts"

serve(async (req: Request) => {
    return await fetch("https://www.yuehwa.com/zh-hk/companyprofile");
});
