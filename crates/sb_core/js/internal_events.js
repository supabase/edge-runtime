
const _internalEvent = (name, details) => {
    return new CustomEvent(name, {
        bubbles: true,
        detail: {
            ...details
        }
    });
}

const Boot = (details) => {
    return _internalEvent("boot", details);
}

const BootFailure = (details) => {
    return _internalEvent("bootFailure", details);
}

const InternalEvents = {
    Boot,
    BootFailure
}

export { InternalEvents };