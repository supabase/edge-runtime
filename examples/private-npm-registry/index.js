import foobar from "npm:@meowmeow/foobar";

console.log(foobar());

export default {
    fetch() {
        return Response.json({
            meow: typeof foobar
        });
    }
}