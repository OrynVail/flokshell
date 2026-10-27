import QtQuick
import Quickshell
import Quickshell.Io
import qs.modules.services
import qs.modules.globals

QtObject {
    id: root

    function generate(Colors) {
        console.log("PywalGenerator: generate() called")
        if (!Colors) {
            console.error("PywalGenerator: Colors is null/undefined")
            return
        }

        try {
            const fmt = (c) => c.toString()
            
            // Safely get wallpaper image
            let image = ""
            if (typeof GlobalStates !== "undefined" && GlobalStates.wallpaperManager) {
                image = GlobalStates.wallpaperManager.currentWallpaper || ""
            } else {
                console.warn("PywalGenerator: GlobalStates.wallpaperManager unavailable. Wallpaper path will be empty.")
            }

            console.log("PywalGenerator: Using wallpaper:", image)

            
            // Helper to darken color (percent 0-100)
            const darken = (c, percent) => {
                try {
                    // Qt.tint takes (source, tintColor). 
                    // To darken, we tint with black having alpha = percent/100.
                    // Qt.rgba(r,g,b,a) takes values 0.0-1.0
                    return Qt.tint(c, Qt.rgba(0, 0, 0, percent / 100)).toString()
                } catch (err) {
                    console.error("PywalGenerator: Error darkening color:", c, err)
                    return "#000000"
                }
            }

            // 1. ~/.cache/wal/colors
            let c0 = fmt(Colors.background)       
            let c1 = fmt(Colors.surfaceVariant)   
            let c2 = fmt(Colors.red)              
            let c3 = fmt(Colors.lightRed)         
            let c4 = fmt(Colors.green)            
            let c5 = fmt(Colors.lightGreen)       
            let c6 = fmt(Colors.yellow)           
            let c7 = fmt(Colors.lightYellow)      
            let c8 = fmt(Colors.primary)          
            let c9 = fmt(Colors.lightBlue)        
            let c10 = fmt(Colors.magenta)         
            let c11 = fmt(Colors.lightMagenta)    
            let c12 = fmt(Colors.cyan)            
            let c13 = fmt(Colors.lightCyan)       
            let c14 = fmt(Colors.overSurface)     
            let c15 = fmt(Colors.overSurface)     

            let colorsContent = [c0, c1, c2, c3, c4, c5, c6, c7, c8, c9, c10, c11, c12, c13, c14, c15].join("\n")

            // 2. ~/.cache/wal/colors.json
            const jsonColors = {
                "wallpaper": image,
                "alpha": "100",
                "special": {
                    "background": darken(Colors.background, 5.0),
                    "foreground": fmt(Colors.overBackground),
                    "cursor": fmt(Colors.surfaceBright)
                },
                "colors": {
                    "color0": fmt(Colors.background),
                    "color1": fmt(Colors.surfaceVariant),
                    "color2": fmt(Colors.secondaryFixedDim),
                    "color3": fmt(Colors.outline),
                    "color4": fmt(Colors.overSurfaceVariant),
                    "color5": fmt(Colors.overSurface),
                    "color6": fmt(Colors.overSurface),
                    "color7": fmt(Colors.surface),
                    "color8": darken(Colors.error, 10.0),
                    "color9": fmt(Colors.tertiary),
                    "color10": fmt(Colors.primary),
                    "color11": fmt(Colors.tertiaryFixed),
                    "color12": fmt(Colors.primaryFixedDim),
                    "color13": fmt(Colors.surfaceBright),
                    "color14": fmt(Colors.overPrimaryContainer),
                    "color15": fmt(Colors.overSurface)
                }
            }
            const jsonContent = JSON.stringify(jsonColors, null, 2)

            // 3. ~/.cache/wal/colors.sh
            let shContent = ""
            shContent += `color0="${fmt(Colors.surface)}"\n`
            shContent += `color1="${darken(Colors.primary, 12.5)}"\n`
            shContent += `color2="${darken(Colors.primary, 10.0)}"\n`
            shContent += `color3="${darken(Colors.primary, 7.5)}"\n`
            shContent += `color4="${darken(Colors.primary, 5.0)}"\n`
            shContent += `color5="${darken(Colors.primary, 2.5)}"\n`
            shContent += `color6="${fmt(Colors.primary)}"\n`
            shContent += `color7="${fmt(Colors.overSurfaceVariant)}"\n`
            shContent += `color8="${fmt(Colors.surfaceVariant)}"\n`
            shContent += `color9="${darken(Colors.primaryFixed, 12.5)}"\n`
            shContent += `color10="${darken(Colors.primaryFixed, 10.0)}"\n`
            shContent += `color11="${darken(Colors.primaryFixed, 7.5)}"\n`
            shContent += `color12="${darken(Colors.primaryFixed, 5.0)}"\n`
            shContent += `color13="${darken(Colors.primaryFixed, 2.5)}"\n`
            shContent += `color14="${fmt(Colors.primaryFixed)}"\n`
            shContent += `color15="${fmt(Colors.overSurface)}"\n`

            // 4. ~/.cache/wal/wal
            // image variable

            const home = Quickshell.env("HOME")
            const walDir = home + "/.cache/wal"

            
            FileWriter.write(walDir + "/colors", colorsContent + "\n")
            FileWriter.write(walDir + "/colors.json", jsonContent + "\n")
            FileWriter.write(walDir + "/colors.sh", shContent + "\n")
            FileWriter.write(walDir + "/wal", image + "\n", ok => {
                if (!ok)
                    return;
                writerProcess.command = ["sh", "-c", "pywalfox update & walogram -B > /dev/null 2>&1 &"];
                writerProcess.running = true;
            })
            
        } catch (e) {
            console.error("PywalGenerator: Critical error in generate:", e)
        }
    }

    property Process writerProcess: Process {
        id: writerProcess
        running: false
        stdout: StdioCollector {
            onStreamFinished: console.log("PywalGenerator: Colors generated.")
        }
        stderr: StdioCollector {
            onStreamFinished: (err) => {
                if (err) console.error("PywalGenerator Error:", err)
            }
        }
    }
}
