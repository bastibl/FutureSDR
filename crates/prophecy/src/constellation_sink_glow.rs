use futures::StreamExt;
use gloo_net::websocket::{futures::WebSocket, Message};
use leptos::html::Canvas;
use leptos::logging::*;
use leptos::*;
use num_complex::Complex32;
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::JsCast;
use web_sys::WebGlBuffer;
use web_sys::WebGlProgram;
use web_sys::WebGl2RenderingContext as GL;

pub const BINS: usize = 128;

struct RenderState {
    canvas: HtmlElement<Canvas>,
    gl: GL,
    width: MaybeSignal<f32>,
    shader: WebGlProgram,
    texture: [f32; BINS * BINS],
}

#[component]
pub fn ConstellationSinkGlow(
    #[prop(into)] width: MaybeSignal<f32>,
    #[prop(optional, into, default = "ws://127.0.0.1:9002".to_string())] websocket: String,
) -> impl IntoView {
    let data = Rc::new(RefCell::new(None));
    {
        let data = data.clone();
        spawn_local(async move {
            let mut ws = WebSocket::open(&websocket).unwrap();
            while let Some(msg) = ws.next().await {
                match msg {
                    Ok(Message::Bytes(b)) => {
                        *data.borrow_mut() = Some(b);
                    }
                    _ => {
                        log!("ConstellationSinkGlow: WebSocket {:?}", msg);
                    }
                }
            }
            log!("ConstellationSinkGlow: WebSocket Closed");
        });
    }

    let canvas_ref = create_node_ref::<Canvas>();
    canvas_ref.on_load(move |canvas_ref| {
        let _ = canvas_ref.on_mount(move |canvas| {
            let gl: GL = canvas
                .get_context("webgl2")
                .unwrap()
                .unwrap()
                .dyn_into()
                .unwrap();

            let vert_code = r"
                attribute vec2 texCoord;
                varying vec2 coord;

                void main(void) {
                    gl_Position = vec4(texCoord, 0, 1);
                    coord = texCoord;
                }
            ";

            let vert_shader = gl.create_shader(GL::VERTEX_SHADER).unwrap();
            gl.shader_source(&vert_shader, vert_code);
            gl.compile_shader(&vert_shader);

            let frag_code = r"
                precision mediump float;

                varying vec2 coord;
                uniform sampler2D sampler;

                vec3 color_map(float t) {
                    const vec3 c0 = vec3(0.2777273272234177, 0.005407344544966578, 0.3340998053353061);
                    const vec3 c1 = vec3(0.1050930431085774, 1.404613529898575, 1.384590162594685);
                    const vec3 c2 = vec3(-0.3308618287255563, 0.214847559468213, 0.09509516302823659);
                    const vec3 c3 = vec3(-4.634230498983486, -5.799100973351585, -19.33244095627987);
                    const vec3 c4 = vec3(6.228269936347081, 14.17993336680509, 56.69055260068105);
                    const vec3 c5 = vec3(4.776384997670288, -13.74514537774601, -65.35303263337234);
                    const vec3 c6 = vec3(-5.435455855934631, 4.645852612178535, 26.3124352495832);

                    return c0+t*(c1+t*(c2+t*(c3+t*(c4+t*(c5+t*c6)))));
                }

                void main(void) {
                    vec4 sample = texture2D(sampler, vec2(coord.x * 0.5 + 0.5, coord.y * 0.5 - 0.5));
                    gl_FragColor = vec4(color_map(clamp(sample.r, 0.0, 1.0)), 0.9);
                }
            ";

            let frag_shader = gl.create_shader(GL::FRAGMENT_SHADER).unwrap();
            gl.shader_source(&frag_shader, frag_code);
            gl.compile_shader(&frag_shader);

            let shader = gl.create_program().unwrap();
            gl.attach_shader(&shader, &vert_shader);
            gl.attach_shader(&shader, &frag_shader);
            gl.link_program(&shader);
            gl.use_program(Some(&shader));

            let texture = gl.create_texture().unwrap();
            gl.bind_texture(GL::TEXTURE_2D, Some(&texture));
            gl.tex_parameteri(GL::TEXTURE_2D, GL::TEXTURE_WRAP_S, GL::REPEAT as i32);
            gl.tex_parameteri(GL::TEXTURE_2D, GL::TEXTURE_WRAP_T, GL::REPEAT as i32);
            gl.tex_parameteri(GL::TEXTURE_2D, GL::TEXTURE_MIN_FILTER, GL::NEAREST as i32);
            gl.tex_parameteri(GL::TEXTURE_2D, GL::TEXTURE_MAG_FILTER, GL::NEAREST as i32);

            let pbo = gl.create_buffer().unwrap();
            gl.bind_buffer(GL::PIXEL_UNPACK_BUFFER, Some(&pbo));
            let bytes = vec![0; BINS * BINS * 4];
            gl.buffer_data_with_u8_array(GL::PIXEL_UNPACK_BUFFER, &bytes, GL::DYNAMIC_DRAW);

            gl.tex_image_2d_with_i32_and_i32_and_i32_and_format_and_type_and_i32(
                GL::TEXTURE_2D,
                0,
                GL::R32F as i32,
                BINS as i32,
                BINS as i32,
                0,
                GL::RED,
                GL::FLOAT,
                0
            ).unwrap();

            let vertexes = vec![-1.0, -1.0, -1.0, 1.0, 1.0, 1.0, 1.0, -1.0];

            let vertex_buffer = gl.create_buffer().unwrap();
            gl.bind_buffer(GL::ARRAY_BUFFER, Some(&vertex_buffer));
            let array_buffer = js_sys::Float32Array::from(vertexes.as_slice()).buffer();
            gl.buffer_data_with_opt_array_buffer(
                GL::ARRAY_BUFFER,
                Some(&array_buffer),
                GL::STATIC_DRAW,
            );

            let indices = vec![0, 1, 2, 0, 2, 3];
            let num_indices = indices.len() as i32;

            let indices_buffer = gl.create_buffer().unwrap();
            gl.bind_buffer(GL::ELEMENT_ARRAY_BUFFER, Some(&indices_buffer));
            let array_buffer = js_sys::Uint16Array::from(indices.as_slice()).buffer();
            gl.buffer_data_with_opt_array_buffer(
                GL::ELEMENT_ARRAY_BUFFER,
                Some(&array_buffer),
                GL::STATIC_DRAW,
            );

            let texture = [0f32; BINS * BINS];

            let state = Rc::new(RefCell::new(RenderState {
                canvas, gl, shader, texture, width,
            }));
            request_animation_frame(render(state, data))
        });
    });

    view! {
        <canvas node_ref=canvas_ref style="width: 100%; height: 100%" />
    }
}

fn render(
    state: Rc<RefCell<RenderState>>,
    data: Rc<RefCell<Option<Vec<u8>>>>,
) -> impl FnOnce() + 'static {
    move || {
        {
            // log!("render");
            let RenderState {
                canvas,
                gl,
                shader,
                texture,
                width,
            } = &mut (*state.borrow_mut());

            let display_width = canvas.client_width() as u32;
            let display_height = canvas.client_height() as u32;

            let need_resize = canvas.width() != display_width || canvas.height() != display_height;

            if need_resize {
                canvas.set_width(display_width);
                canvas.set_height(display_height);
                gl.viewport(0, 0, display_width as i32, display_height as i32);
            }

            if let Some(bytes) = data.borrow_mut().take() {
                // log!("got data");
                let samples = unsafe {
                    let s = bytes.len() / 8;
                    let p = bytes.as_ptr();
                    std::slice::from_raw_parts(p as *const Complex32, s)
                };
                
                // log!("decay");
                let decay = 0.999f32.powi(samples.len() as i32);
                texture.iter_mut().for_each(|v| *v *= decay);
                // log!("width");
                let width = width.get_untracked();

                // log!("bins");
                for s in samples.into_iter() {
                    let w = ((s.re + width) / (2.0 * width) * BINS as f32).round() as i64;
                    if w >= 0 && w < BINS as i64 {
                        let h = ((s.im + width) / (2.0 * width) * BINS as f32).round() as i64;
                        if h >= 0 && h < BINS as i64 {
                            texture[h as usize * BINS + w as usize] += 0.1;
                        }
                    }
                }

                // for w in 0..BINS {
                //     for h in 0..BINS {
                //         texture[w * BINS + h] = w as f32 / BINS as f32;
                //     }
                // }

                let bytes = unsafe {
                    std::slice::from_raw_parts(texture.as_ptr() as *const u8, BINS * BINS * 4)
                };

                // log!("pub");
                // ===== prepare texture
                gl.buffer_data_with_u8_array(GL::PIXEL_UNPACK_BUFFER, bytes, GL::DYNAMIC_DRAW);

                // log!("tex");
                gl.tex_sub_image_2d_with_i32_and_i32_and_u32_and_type_and_i32(
                    GL::TEXTURE_2D,
                    0,
                    0,
                    0,
                    BINS as i32,
                    BINS as i32,
                    GL::RED,
                    GL::FLOAT,
                    0,
                )
                .unwrap();

                let loc = gl.get_attrib_location(shader, "texCoord") as u32;
                gl.enable_vertex_attrib_array(loc);
                gl.vertex_attrib_pointer_with_i32(loc, 2, GL::FLOAT, false, 0, 0);

                gl.draw_elements_with_i32(GL::TRIANGLES, 6, GL::UNSIGNED_SHORT, 0);
            }
        }
        request_animation_frame(render(state, data))
    }
}

