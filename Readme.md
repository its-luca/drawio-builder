# Drawio Builder

This tool helps to integrate drawio figures into Latex documents and Latex Beamer slides,
especially if you want to use animations.

Its key feature is that it incrementally export layers from your drawio figures which allows you to build simple animations effortlessly.

## Installation

1) Download and install [drawio](https://github.com/jgraph/drawio)
2) Download drawio-builder from one of the releases or build it yourself

## Usage

You can either call the tool manually or integrate it with Latex Workshop in VSCode.
In any case, the tool will check the file timestamps and only rebuild figures if the drawio file is more recent than the exported figures.
See [this video](https://youtu.be/blwoj8HgRDo), for a short overview of the workflow.

### Manual Build

`drawio-builder -i <path to folder with your .drawio figures > -o <path to folder where exported images should be created>`

If `drawio` is not in path, you can specify the binary location with `--drawio <path to drawio binary>`

### VSCode Latex Workshop

If you use VSCode with the Latex Workshop extension, you can add `drawio-builder` as a build step.
See `examples/settings.json` for an example.
Afterwards, your figures will automatically rebuild whenever you change the drawio file.

### Advanced usage

By default, `drawio-builder` will incrementally export the layers of a figure, i.e., if your figure has three layers it fill first export only layer 0, then layers 0,1 and then layers 0,1,2.
If you want to override this behavior for certain figures, you can use the  `--config` with a dedicated json config file.
See `test-data/custom_config.json` for an example.

### Gotchas

- You always need to rename the default "Background" layer to something else. Otherwise, it won't get picked up during the export
- "Higher" layers are always displayed above lower layers. To work around this you might want to manually specify an export order (See Advanced usage).
- To prevent figures from changing size when unveiling new elements, place an invisible rectangle on the first layer. Please let me know if you find a better workaround.