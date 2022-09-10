/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */

package software.amazon.smithy.rust.codegen.client.smithy.generators

import org.junit.jupiter.api.Test
import software.amazon.smithy.model.shapes.StructureShape
import software.amazon.smithy.rust.codegen.client.rustlang.RustModule
import software.amazon.smithy.rust.codegen.client.smithy.generators.error.CombinedErrorGenerator
import software.amazon.smithy.rust.codegen.client.smithy.transformers.OperationNormalizer
import software.amazon.smithy.rust.codegen.client.testutil.TestWorkspace
import software.amazon.smithy.rust.codegen.client.testutil.asSmithyModel
import software.amazon.smithy.rust.codegen.client.testutil.compileAndTest
import software.amazon.smithy.rust.codegen.client.testutil.renderWithModelBuilder
import software.amazon.smithy.rust.codegen.client.testutil.testSymbolProvider
import software.amazon.smithy.rust.codegen.client.testutil.unitTest
import software.amazon.smithy.rust.codegen.client.util.lookup

class CombinedErrorGeneratorTest {
    private val baseModel = """
        namespace error

        operation Greeting {
            errors: [InvalidGreeting, ComplexError, FooException, Deprecated]
        }

        @error("client")
        @retryable
        structure InvalidGreeting {
            message: String,
        }

        @error("server")
        structure FooException { }

        @error("server")
        structure ComplexError {
            abc: String,
            other: Integer
        }

        @error("server")
        @deprecated
        structure Deprecated { }
    """.asSmithyModel()
    private val model = OperationNormalizer.transform(baseModel)
    private val symbolProvider = testSymbolProvider(model)

    @Test
    fun `generates combined error enums`() {
        val project = TestWorkspace.testProject(symbolProvider)
        project.withModule(RustModule.public("error")) { writer ->
            listOf("FooException", "ComplexError", "InvalidGreeting", "Deprecated").forEach {
                model.lookup<StructureShape>("error#$it").renderWithModelBuilder(model, symbolProvider, writer)
            }
            val errors = listOf("FooException", "ComplexError", "InvalidGreeting").map { model.lookup<StructureShape>("error#$it") }
            val generator = CombinedErrorGenerator(model, symbolProvider, symbolProvider.toSymbol(model.lookup("error#Greeting")), errors)
            generator.render(writer)

            writer.unitTest(
                name = "generates_combined_error_enums",
                test = """
                    let kind = GreetingErrorKind::InvalidGreeting(InvalidGreeting::builder().message("an error").build());
                    let error = GreetingError::new(kind, aws_smithy_types::Error::builder().code("InvalidGreeting").message("an error").build());
                    assert_eq!(format!("{}", error), "InvalidGreeting: an error");
                    assert_eq!(error.message(), Some("an error"));
                    assert_eq!(error.code(), Some("InvalidGreeting"));
                    use aws_smithy_types::retry::ProvideErrorKind;
                    assert_eq!(error.retryable_error_kind(), Some(aws_smithy_types::retry::ErrorKind::ClientError));

                    // Generate is_xyz methods for errors.
                    assert_eq!(error.is_invalid_greeting(), true);
                    assert_eq!(error.is_complex_error(), false);

                    // Unhandled variants properly delegate message.
                    let error = GreetingError::generic(aws_smithy_types::Error::builder().message("hello").build());
                    assert_eq!(error.message(), Some("hello"));

                    let error = GreetingError::unhandled("some other error");
                    assert_eq!(error.message(), None);
                    assert_eq!(error.code(), None);

                    // Indicate the original name in the display output.
                    let error = FooError::builder().build();
                    assert_eq!(format!("{}", error), "FooError [FooException]");

                    let error = Deprecated::builder().build();
                    assert_eq!(error.to_string(), "Deprecated");
                """,
            )

            println("file:///${project.baseDir}/src/lib.rs")
            println("file:///${project.baseDir}/src/error.rs")
            project.compileAndTest()
        }
    }
}