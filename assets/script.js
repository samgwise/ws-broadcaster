const messageViewSelector = "#message-log"
const testButtonSelector = "#test-button"

function connectWS() {
    return new WebSocket('ws://localhost:3000/ws');
}

let socket = connectWS();

socket.addEventListener('open', function (event) {
    socket.send('Hello Server!');
    console.info("WebSocket Ready.")

    window.setInterval(() => socket.ping("Logger"), 30000)
});

socket.addEventListener('close', () => {
    window.setTimeout(() => {
        connectWS()
    }, 100)
})

window.onload = () => {

    // Setup message logging view
    let loggingView = document.querySelector(messageViewSelector);

    if (loggingView !== null) {
        socket.addEventListener('message', function (event) {
            console.log('Message from server ', event.data);
            loggingView.appendChild(newLogViewEntry(event.data))
        });
    }
    else {
        console.error(`Failed selecting DOM element for message display with selector "${messageViewSelector}"`)
    }

    // Setup test button
    let testButton = document.querySelector(testButtonSelector)

    if (testButton !== null) {
        testButton.addEventListener('click', () => {
            socket.send("Testing 1, 2, 3")
        })
    }
    else {
        console.error(`Failed selecting DOM element for test message button with selector "${testButtonSelector}"`)
    }

    console.info("Logger UI Ready.")
}

function newLogViewEntry(text) {
    let code = document.createElement("code")
    code.innerText = text
    let item = document.createElement("li")
    item.appendChild(code)

    return item
}