import {CONSTANT} from "./some-other-folder/hello.ts";

export const handler = async (req) => {
    const { name } = await req.json()
    const data = {
        message: `Hello World ${name}! ${CONSTANT}`,
    }
    return new Response(JSON.stringify(data), {
        headers: {
            'Content-Type': 'application/json',
        },
    })
}