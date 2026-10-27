<!-- Classes And Methods - The Media Kit - BParameterGroup -->

## BParameterGroup

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `media/ParameterGroup.h`  
Library:| `libmedia.so`  
Allocation:| Constructor only  
[Class Overview](BParameterGroup_Overview.md "BParameterGroup")

  * [Constructor and Destructor](BParameterGroup.md#BParameterGroup_ConstructorDestructor)
  * [Member Functions](BParameterGroup.md#BParameterGroup_MemberFunctions)

  
  
* * *

### Constructor and Destructor

#### BParameterGroup()

`BParameterGroup();`

Rather than directly creating a `BParameterGroup` object via the new operator, you should call [`BParameterWeb::MakeGroup()`](BParameterWeb.md#BParameterWeb_MakeGroup "MakeGroup\(\)") to create a top-level group, or [`BParameterGroup::MakeGroup()`](BParameterGroup.md#BParameterGroup_MakeGroup "MakeGroup\(\)") if you want to create a subgroup.

#### ~BParameterGroup()

`~BParameterGroup();`

You don't need to delete a `BParameterGroup` object; the [`BParameterWeb`](BParameterWeb.md "BParameterWeb") that owns it will do this for you when the web is deleted.

* * *

### Member Functions

#### CountGroups()

`int32 CountGroups();`

Returns the number of subgroups in the group. This count doesn't subgroups nested within subgroups.

#### CountParameters()

`int32 CountParameters();`

Returns the number of [`BParameter`](BParameter.md "BParameter")s in the group. This count doesn't include [`BParameter`](BParameter.md "BParameter")s in subgroups.

#### GroupAt()

`BParameterGroup* GroupAt(int32 index);`

Returns the subgroup at the specified index within the `BParameterGroup`. If the `index` is negative, or is greater than [`CountGroups()`](BParameterGroup.md#BParameterGroup_CountGroups "CountGroups\(\)")-1, `NULL` is returned.

#### MakeNullParameter()

`[`BNullParameter`](BNullParameter.md "BNullParameter")* MakeNullParameter(int32 id,  
media_type type,  
const char* name,  
const char* kind);`

Creates a new [`BNullParameter`](BNullParameter.md "BNullParameter") within the group, with the internal ID specified by `id`, which should be unique within the [`BParameterWeb`](BParameterWeb.md "BParameterWeb") that owns the group.

The type of media data that travels through the null parameter is indicated by `type`, which could be `B_MEDIA_UNKNOWN_TYPE` or `B_MEDIA_NO_TYPE`. The parameter's `name` will be displayed by client applications that display information about the node, and the parameter has the specified `kind`.

Constant| Description  
---|---  
`B_WEB_PHYSICAL_INPUT`| The parameter represents a physical input (such as a microphone jack or line input jack).  
`B_WEB_PHYSICAL_OUTPUT`| The parameter represents a physical output (such as a line output jack or headphone jack).  
`B_WEB_LOGICAL_INPUT`| The parameter represents a point at which bits of data are transferred between the computer and the A/V input hardware.  
`B_WEB_LOGICAL_OUTPUT`| The parameter represents a point at which bits of data are transferred between the computer and the A/V output hardware.  
`B_WEB_ADC_CONVERTER`| The parameter represents an analog to digital converter.  
`B_WEB_DAC_CONVERTER`| The parameter represents a digital to analog converter.  
`B_WEB_BUFFER_INPUT`| The parameter represents a [media_input](TheMediaKit_DefinedTypes.md#media_input "media_input").  
`B_WEB_BUFFER_OUTPUT`| The parameter represents a [media_output](TheMediaKit_DefinedTypes.md#media_output "media_output").  
`B_GENERIC`| The parameter's kind isn't one of the above.  
  
These kinds are used when a control panel needs to draw a signal flow diagram, so it can use the appropriate symbols for these points in the flow.

Note

![Note](./images/admonitions/Info_32.png)

If you create your own kind, you won't break anything, but you should contact Be Developer Support for guidance, so we can standardize given your needs.

#### MakeDiscreteParameter()

`[`BDiscreteParameter`](BDiscreteParameter.md "BDiscreteParameter")* MakeDiscreteParameter(int32 id,  
media_type type,  
const char* name,  
const char* kind);`

Creates a new [`BDiscreteParameter`](BDiscreteParameter.md "BDiscreteParameter") object and attaches it to the group. The [`BDiscreteParameter`](BDiscreteParameter.md "BDiscreteParameter") will have the specified internal `id`, which should be unique within the [`BParameterWeb`](BParameterWeb.md "BParameterWeb") that owns the group.

The [`BDiscreteParameter`](BDiscreteParameter.md "BDiscreteParameter") will affect media data of the specified `type`, and will have the specified `name`.

The kind of discrete parameter is specified by `kind`, and may be any of the following values:

Constant| Description  
---|---  
`B_MUTE`| The parameter represents a mute control; a value of 0 passes data through unchanged, while a value of 1 mutes the data.  
`B_ENABLE`| The parameter represents an enable toggle; a value of 0 disables the function while a value of 1 enables it.  
`B_INPUT_MUX`| The parameter represents an input MUX. The value specifies which input should pass through.  
`B_OUTPUT_MUX`| The parameter represents an output MUX; the value specifies which output should receive the incoming data.  
`B_TUNER_CHANNEL`| The parameter represents a channel tuner (like a TV channel); the value indicates the channel number.  
`B_TRACK`| The parameter's value indicates a track number.  
`B_RECSTATE`| The parameter indicates whether the node is silent (0), playing (1), or recording (2).  
`B_SHUTTLE_MODE`| The parameter indicates performance mode: -1 for backwards, 0 for stop, 1 for playing, and 2 for paused.  
`B_RESOLUTION`| The parameter indicates video or audio resolution.  
`B_COLOR_SPACE`| The parameter indicates video color space.  
`B_FRAME_RATE`| The parameter represents a selector for picking frame rate.  
  
#### MakeContinuousParameter()

`[`BContinuousParameter`](BContinuousParameter.md "BContinuousParameter")* MakeContinuousParameter(int32 id,  
media_type type,  
const char* name,  
const char* kind,  
const char* unit,  
float minValue,  
float maxValue,  
float step);`

Creates a new [`BContinuousParameter`](BContinuousParameter.md "BContinuousParameter") object and attaches it to the group. The [`BContinuousParameter`](BContinuousParameter.md "BContinuousParameter") will have the specified internal `id`, which should be unique within the [`BParameterWeb`](BParameterWeb.md "BParameterWeb") that owns the group.

The [`BContinuousParameter`](BContinuousParameter.md "BContinuousParameter") will affect media data of the specified `type`, and will have the specified `name`.

The kind of parameter the [`BContinuousParameter`](BContinuousParameter.md "BContinuousParameter") represents is specified by `kind`, and may be any of the following values:

Constant| Description  
---|---  
`B_MASTER_GAIN`| The parameter represents the main volume control.  
`B_GAIN`| The parameter represents a gain control.  
`B_BALANCE`| The parameter represents a balance control.  
`B_FREQUENCY`| The parameter represents a frequency, like a radio tuner.  
`B_LEVEL`| The parameter represents a level, as in EQ and effects.  
`B_SHUTTLE_SPEED`| The parameter represents playback speed. A value of 1.0 indicates normal speed; less than 1.0 is slower, greater than 1.0 is faster.  
`B_CROSSFADE`|  The parameter indicates a crossfade for mixing audio or video; 0 indicates that the the first of a pair of streams should be presented, 100 indicates that the other should be presented, and values in between indicate the degree of mixing that should occur.  
`B_COMPRESSION`| The parameter indicates compression ratio; the value is 0 for no compression, 99 for 100:1 compression.  
`B_QUALITY`| The parameter indicates quality level; the value is 0 for maximum compression, 100 for no compression.  
`B_GOP_SIZE`| The parameter indicates a "group of pictures" such as how many frames apart a keyframe should be inserted into a video stream.  
`B_TUNER_CHANNEL`| The parameter represents a channel tuner (like a TV channel); the value indicates the channel number.  
  
The units of measurement are specified by `unit`, and the parameter's value can range from `minValue` to `maxValue`, with a granularity of `step`.

#### MakeGroup()

`BParameterGroup MakeGroup(const char* name);`

Creates a sub-group with the specified `name`

#### Name()

`const char* Name();`

Returns the name of the `BParameterGroup`. This name can be displayed to the user; for example, it could be used as the label of a [`BTab`](BTab.md "BTab").

#### ParameterAt()

`[`BParameter`](BParameter.md "BParameter")* ParameterAt(int32 index);`

Returns the [`BParameter`](BParameter.md "BParameter") at the specified `index` within the `BParameterGroup`. If the `index` is negative, or is greater than [`CountParameters()`](BParameterGroup.md#BParameterGroup_CountParameters "CountParameters\(\)")-1, `NULL` is returned.

#### Web()

`[`BParameterWeb`](BParameterWeb.md "BParameterWeb")* Web();`

Returns the [`BParameterWeb`](BParameterWeb.md "BParameterWeb") that owns the group.
