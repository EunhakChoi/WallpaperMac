uniform sampler2D g_Texture0;
uniform float g_Time;
uniform float amount; // {"material":"amount","default":0.025}
uniform float speed; // {"material":"speed","default":1.0}
varying vec2 v_TexCoord;
void main(){vec2 uv=v_TexCoord;uv.x+=sin(uv.y*18.0+g_Time*speed)*amount;gl_FragColor=texture2D(g_Texture0,uv);}
