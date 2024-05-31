const messageViewSelector = "#message-log"
const testButtonSelector = "#test-button"
const wsURL = `${ isSecure() ? "wss" : "ws" }://${ window.location.host }/ws`
let connectionAttempts = 0;

function isSecure() {
    return (window.location.protocol === "https:") ? true : false
}

function connectWS() {
    console.info(`Connecting to: ${wsURL}`)
    return new WebSocket(wsURL);
}

let socket = connectWS();

socket.addEventListener("error", (event) => {
    console.error("[WebSocket Error]", event)
});

socket.addEventListener('open', function (event) {
    socket.send('Hello Server!');
    console.info("WebSocket Ready.")
    connectionAttempts = 0
});

function reconnectWS() {
    if (socket.readyState === socket.CLOSED) {
        console.info(`Reconnecting, attempt ${ ++connectionAttempts }...`)
        socket = connectWS()
    }
}

// Enable reconnect check
// This is primarily to cover retry connection logic if the first connection or reconnection attempt fails.
window.setInterval(reconnectWS, 3000)

socket.addEventListener('close', () => {
    console.info("WebSocket disconnected, attempting to reconnect...")

    window.setTimeout(reconnectWS, 100)
})

window.onload = () => {

    // Setup message logging view
    let loggingView = document.querySelector(messageViewSelector);

    if (loggingView !== null) {
        socket.addEventListener('message', function (event) {
            console.log('Message from server ', event.data);
            let newLog = newLogViewEntry(event.data)
            loggingView.appendChild(newLog)
            newLog.scrollIntoView()
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