const http = require("http");
const isOdd = require("is-odd");

console.log(require);
console.log(isOdd(33));

const meow = http.createServer((_, resp) => {
  console.log("meow");
  resp.writeHead(200, {
    "content-type": "text-plain",
  });
  resp.write("meow\n");
  resp.end();
});

meow.listen(8080);
