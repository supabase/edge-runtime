import express from "npm:express@4.18.2";

const app = express();

app.get(/(.*)/, (req, res) => {
    res.send("Welcome to the Dinosaur API!");
});

app.listen(8000);