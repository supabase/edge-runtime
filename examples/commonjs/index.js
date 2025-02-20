const http = require("http");

console.log(require);

http.createServer((_, resp) => {
  resp.writeHead(200, {
    "content-type": "text-plain",
  });
  resp.write("meow\n");
  resp.end();
});
