const MATHJAX_ROOT_PATH = "/web/assets/vendor/mathjax";
const MATHJAX_SCRIPT_URL = `${MATHJAX_ROOT_PATH}/tex-svg.js`;

const pendingFrameIds = new Map();

let mathJaxLoadPromise = null;
let mathJaxUnavailable = false;

export function scheduleMathTypesetting(element) {
    if (!element || pendingFrameIds.has(element)) {
        return;
    }
    if (!element.querySelector("[data-math-source]")) {
        return;
    }

    const timeoutId = window.setTimeout(() => {
        pendingFrameIds.delete(element);
        if (!element.isConnected) {
            return;
        }
        void typesetMathInElement(element);
    }, 0);

    pendingFrameIds.set(element, timeoutId);
}

export function clearMathTypesetting(element) {
    if (!element) {
        return;
    }

    cancelPendingTypesetting(element);

    if (!window.MathJax?.typesetClear) {
        return;
    }
    if (!element.querySelector("mjx-container,[data-math-rendered='true']")) {
        return;
    }

    try {
        window.MathJax.typesetClear([element]);
    } catch (error) {
        console.warn("Failed to clear previous math rendering.", error);
    }
}

async function typesetMathInElement(element) {
    const mathNodes = Array.from(element.querySelectorAll("[data-math-source]"))
        .filter((node) => (
            node.isConnected
            && node.dataset.mathRendered !== "true"
            && node.dataset.mathRendered !== "pending"
        ));
    if (!mathNodes.length) {
        return;
    }

    if (mathJaxUnavailable) {
        mathNodes.forEach(restoreMathFallback);
        return;
    }

    try {
        await ensureMathJaxLoaded();
        mathNodes.forEach(prepareMathNode);
        await window.MathJax.typesetPromise(mathNodes);
        mathNodes.forEach((node) => {
            node.dataset.mathRendered = "true";
            node.classList.remove("is-fallback");
        });
    } catch (error) {
        console.warn("Failed to render math.", error);
        mathNodes.forEach(restoreMathFallback);
    }
}

function prepareMathNode(node) {
    const source = String(node.dataset.mathSource || "");
    const isDisplayMath = node.dataset.mathDisplay === "true";

    node.dataset.mathRendered = "pending";
    node.classList.remove("is-fallback");
    node.textContent = isDisplayMath ? `\\[${source}\\]` : `\\(${source}\\)`;
}

function restoreMathFallback(node) {
    node.dataset.mathRendered = "false";
    node.classList.add("is-fallback");
    node.textContent = String(node.dataset.mathSource || "");
}

function cancelPendingTypesetting(rootElement) {
    for (const [element, timeoutId] of pendingFrameIds.entries()) {
        if (element === rootElement || rootElement.contains(element)) {
            window.clearTimeout(timeoutId);
            pendingFrameIds.delete(element);
        }
    }
}

function ensureMathJaxLoaded() {
    if (window.MathJax?.typesetPromise) {
        return Promise.resolve(window.MathJax);
    }
    if (mathJaxLoadPromise) {
        return mathJaxLoadPromise;
    }

    configureMathJaxGlobal();

    mathJaxLoadPromise = new Promise((resolve, reject) => {
        const existingScript = document.getElementById("mathjax-script");
        if (existingScript) {
            if (window.MathJax?.typesetPromise || window.MathJax?.startup?.promise) {
                waitForMathJaxStartup(resolve, reject);
                return;
            }

            existingScript.addEventListener("load", () => {
                waitForMathJaxStartup(resolve, reject);
            }, { once: true });
            existingScript.addEventListener("error", () => {
                reject(new Error("Failed to load MathJax."));
            }, { once: true });
            return;
        }

        const script = document.createElement("script");
        script.id = "mathjax-script";
        script.async = true;
        script.src = MATHJAX_SCRIPT_URL;
        script.addEventListener("load", () => {
            waitForMathJaxStartup(resolve, reject);
        }, { once: true });
        script.addEventListener("error", () => {
            reject(new Error("Failed to load MathJax."));
        }, { once: true });
        document.head.appendChild(script);
    }).catch((error) => {
        mathJaxUnavailable = true;
        mathJaxLoadPromise = null;
        throw error;
    });

    return mathJaxLoadPromise;
}

function waitForMathJaxStartup(resolve, reject) {
    const startupPromise = window.MathJax?.startup?.promise;
    if (startupPromise && typeof startupPromise.then === "function") {
        startupPromise.then(() => {
            resolve(window.MathJax);
        }).catch(reject);
        return;
    }

    if (window.MathJax?.typesetPromise) {
        resolve(window.MathJax);
        return;
    }

    reject(new Error("MathJax loaded but did not initialize."));
}

function configureMathJaxGlobal() {
    if (window.MathJax?.typesetPromise) {
        return;
    }

    const existingConfig = window.MathJax && typeof window.MathJax === "object"
        ? window.MathJax
        : {};

    window.MathJax = {
        ...existingConfig,
        loader: {
            ...(existingConfig.loader || {}),
            paths: {
                ...(existingConfig.loader?.paths || {}),
                mathjax: MATHJAX_ROOT_PATH,
            },
        },
        svg: {
            fontCache: "local",
            ...(existingConfig.svg || {}),
        },
        options: {
            skipHtmlTags: ["script", "noscript", "style", "textarea", "pre", "code"],
            ...(existingConfig.options || {}),
        },
    };
}
