const { app, BrowserWindow, session, desktopCapturer , Menu} = require("electron");
const path = require("path");
const fs = require("fs");

//should be fixed
app.disableHardwareAcceleration();
app.commandLine.appendSwitch('force-color-profile', 'srgb');

// Webcam still doesnt work
app.commandLine.appendSwitch('disable-features', 'WebRtcHideLocalIpsWithMdns,MediaFoundationVideoCapture');
app.commandLine.appendSwitch('enable-features', 'SharedArrayBuffer');
//app.commandLine.appendSwitch('use-fake-ui-for-media-stream');

let mainWindow;

// get url to server
function getTargetUrl() {
  const defaultUrl = "https://localhost:4991";
  let searchPath;

  if (!app.isPackaged) {
    searchPath = app.getAppPath();
  } else if (process.env.PORTABLE_EXECUTABLE_DIR) {
    searchPath = process.env.PORTABLE_EXECUTABLE_DIR;
  } else {
    searchPath = path.dirname(app.getPath('exe'));
  }

  const configPath = path.join(searchPath, "server.txt");

  try {
    if (fs.existsSync(configPath)) {
      const fileContent = fs.readFileSync(configPath, "utf8").trim();
      if (fileContent.length > 0 && fileContent.startsWith("http")) {
        return fileContent;
      }
    }
  } catch (error) {
    console.error(`[Config] Error: ${error}`);
  }
  
  return defaultUrl;
}

const config = {
  window: {
    backgroundColor: "#1a1a1a",
    width: 1280,
    height: 800,
    title: "Thinfin",
    show: false
  },
  url: getTargetUrl(),
  userAgent: "Thinfin/0.0.1",
  allowedPermissions: ['media', 'videoinput', 'audioinput', 'notifications']
};

app.whenReady().then(() => {
  setupMediaPermissions();
  createWindow();
});

function createWindow() {
  mainWindow = new BrowserWindow({
    width: config.window.width,
    height: config.window.height,
    title: config.window.title,
    backgroundColor: config.window.backgroundColor,
    show: false,
    webPreferences: {
      contextIsolation: true,
      sandbox: true,
      nodeIntegration: false
    }
  });

  mainWindow.loadFile('splash.html');

  mainWindow.once('ready-to-show', () => {
    mainWindow.show();

    mainWindow.loadURL(config.url, {
      userAgent: config.userAgent
    });
  });

  //no
  mainWindow.on('close', (event) => {
    if (!mainWindow) return;
    event.preventDefault();
    mainWindow.destroy();
  });

  mainWindow.on('closed', () => {
    mainWindow = null;
    app.quit();
  });

  mainWindow.setMenuBarVisibility(false);
}

function setupMediaPermissions() {
  const ses = session.defaultSession;

  ses.setPermissionRequestHandler((webContents, permission, callback) => {
    if (config.allowedPermissions.includes(permission)) {
      callback(true);
    } else {
      callback(false);
    }
  });

  ses.setDisplayMediaRequestHandler(async (request, callback) => {
    try {

      const sources = await desktopCapturer.getSources({ 
        types: ["screen", "window"],
        thumbnailSize: { width: 0, height: 0 }
      });

      if (sources.length === 0) return callback({});

      //very bad menu creation, but it works
      const menuItems = sources.map(source => {
        return {
          label: source.name.length > 50 ? source.name.substring(0, 50) + "..." : source.name,
          click: () => {
            callback({ video: source, audio: "loopback" });
          }
        };
      });

      menuItems.push({ type: 'separator' });
      menuItems.push({
        label: 'Cancel',
        click: () => callback({})
      });

      const menu = Menu.buildFromTemplate(menuItems);
      
      menu.popup({ window: mainWindow });

      menu.once('menu-will-close', () => {
        setTimeout(() => {
        }, 100);
      });

    } catch (error) {
      console.error(error);
      callback({});
    }
  }); 

}

app.on("window-all-closed", () => {
  if (process.platform !== "darwin") {
    app.quit();
  }
});